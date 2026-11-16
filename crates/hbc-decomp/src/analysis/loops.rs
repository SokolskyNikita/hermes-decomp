use crate::ir::{BlockId, Terminator, CFG};
use std::collections::{BTreeMap, BTreeSet, HashSet};

// Information about a detected loop.
#[derive(Debug, Clone)]
pub struct LoopInfo {
    pub header: BlockId,
    pub body: BTreeSet<BlockId>,
    pub exit: Option<BlockId>,
    pub back_edges: Vec<(BlockId, BlockId)>,
    pub is_do_while: bool,
}

// Detect all loops in the CFG using back-edge analysis.
//
// Strategy:
// 1. Compute Dominators for the CFG.
// 2. Find Back-Edges: edges `A -> B` where `B` dominates `A`. `B` is the loop header.
// 3. Construct Loop Body: Start from `A` and walk predecessors backwards until reaching `B`.
// 4. Find Loop Exit: Successors of body nodes that are outside the body.
pub fn detect_loops(cfg: &CFG) -> Vec<LoopInfo> {
    let dominators = compute_dominators(cfg);
    let mut loops = Vec::new();
    let mut back_edges = Vec::new();

    // Find back edges (edge to a dominator)
    for block in cfg.blocks() {
        for succ in block.successors() {
            if dominators
                .get(&block.id)
                .map(|d| d.contains(&succ))
                .unwrap_or(false)
            {
                back_edges.push((block.id, succ));
            }
        }
    }

    let mut headers: BTreeMap<BlockId, Vec<(BlockId, BlockId)>> = BTreeMap::new();
    for edge in back_edges {
        headers.entry(edge.1).or_default().push(edge);
    }

    for (header, edges) in headers {
        let mut body = BTreeSet::new();
        body.insert(header);

        for (from, _) in &edges {
            collect_loop_body(cfg, &dominators, *from, header, &mut body);
        }

        // Find loop exit (successor of header not in body, or branch target out of body)
        let exit = find_loop_exit(cfg, header, &body);

        // Determine if this is a do-while (condition at end)
        let is_do_while = edges.iter().any(|(from, to)| {
            *to == header
                && cfg
                    .get(*from)
                    .map(|b| matches!(b.terminator, Terminator::Branch { .. }))
                    .unwrap_or(false)
        });

        loops.push(LoopInfo {
            header,
            body,
            exit,
            back_edges: edges,
            is_do_while,
        });
    }

    loops
}

fn collect_loop_body(
    cfg: &CFG,
    dominators: &BTreeMap<BlockId, HashSet<BlockId>>,
    node: BlockId,
    header: BlockId,
    body: &mut BTreeSet<BlockId>,
) {
    if body.contains(&node) {
        return;
    }
    body.insert(node);
    for pred in cfg.predecessors(node) {
        if pred != header {
            collect_loop_body(cfg, dominators, pred, header, body);
        }
    }
    // A catch block in the loop is entered from its protected blocks. They are
    // part of the loop too (a `throw` inside the try has no normal successor,
    // so the walk above never reaches it), as long as the header dominates
    // them: a try that encloses the whole loop stays outside.
    for pred in cfg.exception_edge_sources(node) {
        let in_loop = dominators.get(&pred).is_some_and(|d| d.contains(&header));
        if pred != header && in_loop {
            collect_loop_body(cfg, dominators, pred, header, body);
        }
    }
}

fn find_loop_exit(cfg: &CFG, header: BlockId, body: &BTreeSet<BlockId>) -> Option<BlockId> {
    if let Some(exit) = common_loop_exit(cfg, body) {
        return Some(exit);
    }
    // The first exit in block order, the header's own first. A block that
    // only throws is taken only when there is nothing else.
    let mut candidates: Vec<BlockId> = Vec::new();
    for b in std::iter::once(header).chain(body.iter().copied()) {
        if let Some(block) = cfg.get(b) {
            candidates.extend(block.successors().into_iter().filter(|s| !body.contains(s)));
        }
    }
    let throws = |b: &BlockId| {
        cfg.get(*b)
            .is_some_and(|blk| matches!(blk.terminator, Terminator::Throw(_)))
    };
    candidates
        .iter()
        .find(|b| !throws(b))
        .or(candidates.first())
        .copied()
}

// When the loop leaves through several blocks and all of them flow into one
// of them, that one is the real exit. This is the shape of a `break` out of a
// `try` with a `finally`: Hermes copies the finally body onto the break path,
// so the break goes to that copy and the copy falls into the block after the
// loop. Picking the copy as the exit (the first exit in block order) left the
// loop's own condition with nowhere to break to, and it was dropped.
fn common_loop_exit(cfg: &CFG, body: &BTreeSet<BlockId>) -> Option<BlockId> {
    let mut exits: Vec<BlockId> = Vec::new();
    for &b in body {
        if let Some(block) = cfg.get(b) {
            for succ in block.successors() {
                if !body.contains(&succ) && !exits.contains(&succ) {
                    exits.push(succ);
                }
            }
        }
    }
    // An exit block that only throws is not where the loop goes on to: with
    // it picked as the exit, `default: throw ..` in a loop's switch became
    // the code after the loop and the real exit became the throw's `break`.
    let before = exits.len();
    let throws = |b: BlockId| {
        cfg.get(b)
            .is_some_and(|blk| matches!(blk.terminator, Terminator::Throw(_)))
    };
    if exits.iter().any(|&e| !throws(e)) {
        exits.retain(|&e| !throws(e));
    }
    if exits.len() == 1 && before > 1 {
        return Some(exits[0]);
    }
    if exits.len() < 2 {
        return None;
    }
    let reaches = |from: BlockId, to: BlockId| -> bool {
        let mut seen: HashSet<BlockId> = HashSet::new();
        let mut stack = vec![from];
        while let Some(n) = stack.pop() {
            if n == to {
                return true;
            }
            if body.contains(&n) || !seen.insert(n) {
                continue;
            }
            if let Some(block) = cfg.get(n) {
                stack.extend(block.successors());
            }
        }
        false
    };
    if let Some(e) = exits
        .iter()
        .copied()
        .find(|&e| exits.iter().all(|&o| o == e || reaches(o, e)))
    {
        return Some(e);
    }
    // No exit is reached from all the others, but they may still meet
    // outside the loop: `if (sib) { ..; return } .. while (c)` leaves through
    // two blocks that both fall into the function's one `return`. That join
    // is the exit, and the two paths to it become the loop's `break`s.
    let mut seen: HashSet<BlockId> = HashSet::new();
    let mut queue = std::collections::VecDeque::from([exits[0]]);
    while let Some(n) = queue.pop_front() {
        if body.contains(&n) || !seen.insert(n) {
            continue;
        }
        if exits.iter().all(|&o| reaches(o, n)) {
            return Some(n);
        }
        if let Some(block) = cfg.get(n) {
            queue.extend(block.successors());
        }
    }
    None
}

pub fn compute_dominators(cfg: &CFG) -> BTreeMap<BlockId, HashSet<BlockId>> {
    let all_blocks: HashSet<_> = cfg.block_ids().collect();
    let mut dom: BTreeMap<BlockId, HashSet<BlockId>> = BTreeMap::new();

    // Exception catch blocks are entered from outside normal control flow (the
    // exception edge is not a CFG successor), so they have no normal predecessors
    // and are unreachable from `entry`. Treat them as additional roots: without
    // this they keep the degenerate "all blocks" dominator set, which makes their
    // own outgoing edge (e.g. `catch -> return`) look like a back edge and
    // spuriously flags the target as a loop header, that dropped the `return`
    // out of `catch (e) { return e.message; }`.
    //
    // A catch block is not a root of its own, though: it is reached from every
    // block of its protected range. Making it a root cut the try blocks out of
    // the dominator sets of everything after the catch, so a loop whose body
    // has a try/catch (`for (..) { try { .. } catch { continue; } }`) lost its
    // back edge and was not recognised as a loop at all. Feed each catch block
    // from its protected blocks instead, and fall back to a root only when the
    // range is empty.
    let mut exception_preds: BTreeMap<BlockId, Vec<BlockId>> = BTreeMap::new();
    for h in &cfg.exception_handlers {
        let preds = exception_preds.entry(h.catch_block).or_default();
        for &b in &h.try_blocks {
            if b != h.catch_block && !preds.contains(&b) {
                preds.push(b);
            }
        }
    }
    let roots: HashSet<BlockId> = std::iter::once(cfg.entry)
        .chain(
            exception_preds
                .iter()
                .filter(|(_, p)| p.is_empty())
                .map(|(c, _)| *c),
        )
        .collect();

    for id in cfg.block_ids() {
        if roots.contains(&id) {
            let mut s = HashSet::new();
            s.insert(id);
            dom.insert(id, s);
        } else {
            dom.insert(id, all_blocks.clone());
        }
    }

    // Fixed-point iteration
    let rpo = cfg.reverse_postorder();
    let mut changed = true;

    while changed {
        changed = false;
        for &block_id in &rpo {
            if roots.contains(&block_id) {
                continue;
            }

            let mut preds = cfg.predecessors(block_id);
            if let Some(extra) = exception_preds.get(&block_id) {
                for &p in extra {
                    if !preds.contains(&p) {
                        preds.push(p);
                    }
                }
            }
            if preds.is_empty() {
                continue;
            }

            let mut new_dom = dom.get(&preds[0]).cloned().unwrap_or_default();
            for &pred in &preds[1..] {
                if let Some(pred_dom) = dom.get(&pred) {
                    new_dom = new_dom.intersection(pred_dom).copied().collect();
                }
            }
            new_dom.insert(block_id);

            if new_dom != *dom.get(&block_id).unwrap() {
                changed = true;
                dom.insert(block_id, new_dom);
            }
        }
    }

    dom
}
