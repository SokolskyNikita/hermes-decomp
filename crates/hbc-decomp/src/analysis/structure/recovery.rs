use super::exceptions::recover_catch_body;
use super::loops::{recover_loop, recover_loop_with_try};
use super::{RecoveryCtx, Structure};
use crate::analysis::loops::{detect_loops, LoopInfo};
use crate::ir::{BlockId, Expression, Statement, Terminator, CFG};
use std::collections::{BTreeMap, HashSet};

// Recovers high-level structures from CFG.
//
// This is the "Structurization" phase.
// The CFG (Control Flow Graph) is a graph of basic blocks with jumps.
// We need to convert this back into nested structures like `if`, `while`, `for`.
//
// Strategy:
// 1. Detect loops (headers, bodies, exits) using graph analysis.
// 2. Traverse the graph starting from entry.
// 3. Recursively match patterns:
//    - If a node dominates 2 children that merge back, it's an `if`.
//    - If a node is a loop header, recurse into the loop body.
//    - Handle `break` and `continue` by checking loop stacks.
pub fn analyze(cfg: &CFG) -> (Structure, Vec<LoopInfo>) {
    let loops = detect_loops(cfg);
    let mut visited = HashSet::new();
    let loop_stack = Vec::new();

    // Build a map: try_block_start → exception handler info.
    //
    // A single source-level `try` can compile to several handler ranges that all
    // target the *same* catch block, the compiler splits the protected region
    // around an inner branch (e.g. the `throw` arm gets its own range). Keying
    // naively by start would then emit one nested try/catch per range. Collapse
    // them: register a single try per catch block, at its earliest start, so the
    // body recovery folds the remaining ranges in as ordinary control flow.
    let mut earliest_for_catch: BTreeMap<BlockId, &crate::ir::CfgExceptionHandler> =
        BTreeMap::new();
    for handler in &cfg.exception_handlers {
        earliest_for_catch
            .entry(handler.catch_block)
            .and_modify(|h| {
                if handler.try_block_start < h.try_block_start {
                    *h = handler;
                }
            })
            .or_insert(handler);
    }
    // Several handlers can start at the same block. Hermes lowers
    // `try { A } catch (e) { B } finally { C }` to a catch handler over `A` and
    // a catch-all handler (`Catch r; C; Throw r`) over `A` and `B`, both starting
    // at `A`'s first block; a `try` whose body opens with another `try` does the
    // same. Keying by start kept only one of them and silently dropped the other
    // (usually the real catch). Keep them all and nest them: the handler that
    // protects more blocks encloses the others. On a tie, the handler listed
    // later in the table is the outer one, as Hermes lists inner handlers first.
    let mut try_starts: BTreeMap<BlockId, Vec<&crate::ir::CfgExceptionHandler>> = BTreeMap::new();
    for handler in &cfg.exception_handlers {
        if let Some(h) = earliest_for_catch.get(&handler.catch_block) {
            if std::ptr::eq(*h, handler) {
                try_starts
                    .entry(handler.try_block_start)
                    .or_default()
                    .push(handler);
            }
        }
    }
    for handlers in try_starts.values_mut() {
        handlers.reverse();
        handlers
            .sort_by_key(|h| std::cmp::Reverse(cfg.exception_edge_sources(h.catch_block).len()));
    }

    let mut ctx = RecoveryCtx {
        cfg,
        loops: &loops,
        visited: &mut visited,
        try_starts: &try_starts,
        plain_header: None,
        switch_loop_depth: None,
    };

    let root = recover_structure(&mut ctx, cfg.entry, &loop_stack);
    (root, loops)
}

pub(super) fn recover_structure(
    ctx: &mut RecoveryCtx<'_>,
    block_id: BlockId,
    loop_stack: &[&LoopInfo],
) -> Structure {
    if ctx.visited.contains(&block_id) {
        // If we're jumping to a loop header we're in, it's a continue
        for (i, loop_info) in loop_stack.iter().enumerate().rev() {
            if block_id == loop_info.header {
                let label = ctx.loop_label(i, loop_stack.len(), false);
                return Structure::Continue(label);
            }
            if loop_info.exit == Some(block_id) {
                let label = ctx.loop_label(i, loop_stack.len(), true);
                return Structure::Break(label);
            }
        }
        return Structure::Block(block_id, vec![]);
    }

    if let Some(handlers) = ctx.try_starts.get(&block_id) {
        let handlers = handlers.clone();
        // The try opens at a loop header and its protected range lies inside
        // the loop: the try is in the loop body (`for (..) { try { .. } }`), so
        // the loop goes outside. Wrapping the loop in the try instead moved
        // the catch out of the loop, and a caught iteration ended the loop.
        if ctx.plain_header != Some(block_id) {
            if let Some(loop_info) = ctx.loops.iter().find(|l| l.header == block_id) {
                let inside = handlers.iter().all(|h| {
                    ctx.cfg
                        .exception_edge_sources(h.catch_block)
                        .iter()
                        .all(|b| loop_info.body.contains(b))
                });
                if inside {
                    let loop_info = loop_info.clone();
                    return recover_loop_with_try(ctx, &loop_info, &handlers, loop_stack);
                }
            }
        }
        return recover_try(ctx, block_id, &handlers, loop_stack);
    }

    recover_structure_inner(ctx, block_id, loop_stack)
}

// Recover the try statements that start at `block_id`, `handlers[0]` outermost.
// The body of each is the try statements for the remaining handlers, and the
// innermost body is the plain block walk.
pub(super) fn recover_try(
    ctx: &mut RecoveryCtx<'_>,
    block_id: BlockId,
    handlers: &[&crate::ir::CfgExceptionHandler],
    loop_stack: &[&LoopInfo],
) -> Structure {
    let Some((handler, inner)) = handlers.split_first() else {
        return recover_structure_inner(ctx, block_id, loop_stack);
    };
    {
        let catch_block_id = handler.catch_block;

        // The protected range has an end. The block the range falls through
        // to is where the try and the catch reconverge, and it belongs after
        // the statement, not inside the try body. Without this fence the body
        // walk kept going and swallowed everything up to the return, and the
        // catch came out empty. The fence is only placed when the exit is
        // unambiguous and is not a loop boundary the walk already handles.
        let region: HashSet<BlockId> = ctx
            .cfg
            .exception_edge_sources(catch_block_id)
            .into_iter()
            .collect();
        let exit = try_region_exit(ctx.cfg, &region, catch_block_id, loop_stack.last().copied())
            .filter(|e| {
                !ctx.visited.contains(e)
                    && !loop_stack
                        .iter()
                        .any(|l| l.header == *e || l.exit == Some(*e))
            });
        if let Some(exit) = exit {
            ctx.visited.insert(exit);
        }

        // Mark catch block as visited to prevent it from being included in try body traversal
        let catch_was_visited = ctx.visited.contains(&catch_block_id);
        ctx.visited.insert(catch_block_id);

        // Recover the try body (catch block excluded via visited set)
        let try_body = recover_try(ctx, block_id, inner, loop_stack);

        // Now recover the catch body, temporarily un-visit the catch block
        if !catch_was_visited {
            ctx.visited.remove(&catch_block_id);
        }
        let (catch_param, catch_body) = recover_catch_body(ctx, catch_block_id, loop_stack);

        let try_catch = Structure::TryCatch {
            try_body: Box::new(try_body),
            catch_param,
            catch_body: Box::new(catch_body),
        };
        match exit {
            Some(exit) => {
                ctx.visited.remove(&exit);
                let after = recover_structure(ctx, exit, loop_stack);
                Structure::Sequence(vec![try_catch, after])
            }
            None => try_catch,
        }
    }
}

pub(super) fn recover_structure_inner(
    ctx: &mut RecoveryCtx<'_>,
    block_id: BlockId,
    loop_stack: &[&LoopInfo],
) -> Structure {
    if ctx.visited.contains(&block_id) {
        for (i, loop_info) in loop_stack.iter().enumerate().rev() {
            if block_id == loop_info.header {
                let label = ctx.loop_label(i, loop_stack.len(), false);
                return Structure::Continue(label);
            }
            if loop_info.exit == Some(block_id) {
                let label = ctx.loop_label(i, loop_stack.len(), true);
                return Structure::Break(label);
            }
        }
        return Structure::Block(block_id, vec![]);
    }

    // Check if this is a loop header
    if ctx.plain_header == Some(block_id) {
        ctx.plain_header = None;
    } else if let Some(loop_info) = ctx.loops.iter().find(|l| l.header == block_id) {
        if !ctx.visited.contains(&block_id) {
            let loop_info = loop_info.clone();
            return recover_loop(ctx, &loop_info, loop_stack);
        }
    }

    ctx.visited.insert(block_id);

    let block = match ctx.cfg.get(block_id) {
        Some(b) => b,
        None => {
            log::warn!(
                "Block {} not found in CFG during structure recovery",
                block_id.0
            );
            return Structure::Block(block_id, vec![]);
        }
    };

    let stmts = block.statements.clone();

    match &block.terminator {
        Terminator::Return(e) => {
            let mut all = stmts;
            all.push(Statement::Return(e.clone()));
            Structure::Block(block_id, all)
        }
        Terminator::Throw(e) => {
            let mut all = stmts;
            all.push(Statement::Throw(e.clone()));
            Structure::Block(block_id, all)
        }
        Terminator::Jump(target) => {
            let target = *target;
            // Check for loop continue/break, emit real Break/Continue statements
            // (not Comment markers). Comment("break")/Comment("continue") previously
            // leaked into codegen as `// break` / `// continue` instead of structured
            // control flow (HBC96 reference bundle: 1268 such comments).
            for (i, loop_info) in loop_stack.iter().enumerate().rev() {
                if target == loop_info.header && ctx.visited.contains(&target) {
                    let label = ctx.loop_label(i, loop_stack.len(), false);
                    let mut stmts = stmts;
                    stmts.push(Statement::Continue(label));
                    return Structure::Block(block_id, stmts);
                }
                if loop_info.exit == Some(target) {
                    let label = ctx.loop_label(i, loop_stack.len(), true);
                    let mut stmts = stmts;
                    stmts.push(Statement::Break(label));
                    return Structure::Block(block_id, stmts);
                }
            }

            let block_struct = Structure::Block(block_id, stmts);
            let next = recover_structure(ctx, target, loop_stack);
            Structure::Sequence(vec![block_struct, next])
        }
        Terminator::Branch {
            condition,
            true_target,
            false_target,
        } => {
            let condition = condition.clone();
            let true_target = *true_target;
            let false_target = *false_target;

            // Check for loop patterns
            if let Some(loop_info) = loop_stack.last() {
                let true_exits = loop_info.exit == Some(true_target);
                let false_exits = loop_info.exit == Some(false_target);

                if true_exits && !false_exits {
                    let mut parts = vec![Structure::Block(block_id, stmts.clone())];
                    let else_ = recover_structure(ctx, false_target, loop_stack);
                    parts.push(Structure::If {
                        condition: condition.clone(),
                        then_: Box::new(Structure::Break(ctx.loop_label(
                            loop_stack.len() - 1,
                            loop_stack.len(),
                            true,
                        ))),
                        else_: Box::new(else_),
                    });
                    return Structure::Sequence(parts);
                }

                if false_exits && !true_exits {
                    let mut parts = vec![Structure::Block(block_id, stmts.clone())];
                    let then_ = recover_structure(ctx, true_target, loop_stack);
                    parts.push(Structure::If {
                        condition: Expression::unary(crate::ir::UnaryOp::Not, condition.clone()),
                        then_: Box::new(Structure::Break(ctx.loop_label(
                            loop_stack.len() - 1,
                            loop_stack.len(),
                            true,
                        ))),
                        else_: Box::new(then_),
                    });
                    return Structure::Sequence(parts);
                }
            }

            // Diamond: if the two branches reconverge at a merge block, recover
            // each branch only UP TO that merge, then emit the merge AFTER the
            // if. Otherwise the merge (the common tail, e.g. a shared `return`)
            // is wrongly absorbed into the `then` branch and the `else` becomes
            // empty.
            if let Some(merge) = find_merge_point(ctx.cfg, block_id, true_target, false_target) {
                if merge != true_target || merge != false_target {
                    let merge_was_visited = ctx.visited.contains(&merge);
                    ctx.visited.insert(merge);
                    let then_ = recover_structure(ctx, true_target, loop_stack);
                    let else_ = recover_structure(ctx, false_target, loop_stack);
                    if !merge_was_visited {
                        ctx.visited.remove(&merge);
                    }
                    let mut parts = vec![
                        Structure::Block(block_id, stmts),
                        Structure::If {
                            condition,
                            then_: Box::new(then_),
                            else_: Box::new(else_),
                        },
                    ];
                    // When the arms meet only at the innermost loop's exit or
                    // header, each arm already ends in its own `break` or
                    // `continue`. Emitting the merge after the `if` would add a
                    // stray `break` that also runs for an arm that falls
                    // through to the next iteration.
                    let loop_edge = loop_stack
                        .last()
                        .is_some_and(|l| l.exit == Some(merge) || l.header == merge);
                    if !loop_edge {
                        parts.push(recover_structure(ctx, merge, loop_stack));
                    }
                    return Structure::Sequence(parts);
                }
            }

            let then_ = recover_structure(ctx, true_target, loop_stack);
            let else_ = recover_structure(ctx, false_target, loop_stack);
            let mut parts = vec![Structure::Block(block_id, stmts)];
            parts.push(Structure::If {
                condition,
                then_: Box::new(then_),
                else_: Box::new(else_),
            });
            Structure::Sequence(parts)
        }
        Terminator::Switch {
            value,
            cases,
            default,
        } => {
            let value = value.clone();
            let cases = cases.clone();
            let default = *default;
            let mut parts = vec![Structure::Block(block_id, stmts)];

            // Recover each case only up to the block the cases rejoin at, and
            // emit that block after the switch. Without it the common tail
            // went into the first case that reached it and every other case
            // ended at the switch, skipping the tail (in a loop, the code
            // that moves to the next iteration).
            let mut targets: Vec<BlockId> = cases.iter().map(|(_, t)| *t).collect();
            targets.push(default);
            let merge = find_switch_merge(ctx.cfg, block_id, &targets, loop_stack.last().copied())
                .filter(|m| !ctx.visited.contains(m));
            if let Some(m) = merge {
                ctx.visited.insert(m);
            }
            // A `break` to the innermost loop's exit from a case would only
            // leave the switch: such breaks carry the loop's label.
            let saved_depth = ctx.switch_loop_depth.replace(loop_stack.len());

            let mut recovered: BTreeMap<BlockId, Structure> = BTreeMap::new();
            for (_case_val, target) in &cases {
                if !recovered.contains_key(target) {
                    let body = recover_structure(ctx, *target, loop_stack);
                    recovered.insert(*target, body);
                }
            }

            let mut switch_cases = Vec::new();
            for (case_val, target) in &cases {
                switch_cases.push((case_val.clone(), recovered[target].clone()));
            }

            let default_body = if let Some(cached) = recovered.get(&default) {
                cached.clone()
            } else {
                recover_structure(ctx, default, loop_stack)
            };
            ctx.switch_loop_depth = saved_depth;

            parts.push(Structure::Switch {
                discriminant: value,
                cases: switch_cases,
                default: Box::new(default_body),
            });
            if let Some(m) = merge {
                ctx.visited.remove(&m);
                parts.push(recover_structure(ctx, m, loop_stack));
            }

            Structure::Sequence(parts)
        }
        Terminator::None => {
            log::debug!(
                "Block {} has Terminator::None, treating as simple block",
                block_id.0
            );
            Structure::Block(block_id, stmts)
        }
    }
}

// The block the cases of a switch rejoin at: of the blocks reachable from the
// case targets (without passing back through the switch), the one reached
// from the most targets, nearest to the switch on a tie. Cases that end by
// returning or throwing reach nothing and do not count. Inside a loop, walks
// stop at the loop's header and at blocks outside the loop, and those are
// never the merge: a case reaching them ends in `continue`/`break`. None when
// fewer than two targets share a block.
fn find_switch_merge(
    cfg: &CFG,
    switch: BlockId,
    targets: &[BlockId],
    innermost_loop: Option<&LoopInfo>,
) -> Option<BlockId> {
    use std::collections::VecDeque;
    let in_scope = |b: BlockId| innermost_loop.is_none_or(|l| b != l.header && l.body.contains(&b));
    let mut distinct: Vec<BlockId> = Vec::new();
    for &t in targets {
        if !distinct.contains(&t) {
            distinct.push(t);
        }
    }
    let mut count: BTreeMap<BlockId, usize> = BTreeMap::new();
    for &t in &distinct {
        let mut seen: HashSet<BlockId> = HashSet::new();
        let mut stack = vec![t];
        while let Some(n) = stack.pop() {
            if n == switch || !in_scope(n) || !seen.insert(n) {
                continue;
            }
            if let Some(blk) = cfg.get(n) {
                stack.extend(blk.successors());
            }
        }
        for n in seen {
            *count.entry(n).or_default() += 1;
        }
    }
    let best = count.values().copied().max().filter(|&c| c >= 2)?;
    // Nearest block with the best count, breadth-first from the switch.
    let mut seen: HashSet<BlockId> = HashSet::new();
    let mut queue: VecDeque<BlockId> = distinct.iter().copied().collect();
    while let Some(n) = queue.pop_front() {
        if n == switch || !in_scope(n) || !seen.insert(n) {
            continue;
        }
        if count.get(&n) == Some(&best) {
            return Some(n);
        }
        if let Some(blk) = cfg.get(n) {
            queue.extend(blk.successors());
        }
    }
    None
}

// Find the merge (join) point of a branch: the nearest block reachable from
// BOTH the true and false targets. This is the block where the two arms of an
// `if`/`else` diamond reconverge (its immediate post-dominator for simple
// diamonds). Returns None when the branches never reconverge (e.g. both return).
//
// `header` is the branch block itself. Both reachability walks stop *before*
// re-entering `header`: a genuine forward join is reached without re-executing
// the branch. Without this guard, a loop back-edge that flows back through the
// branch would make the branch's own arms mutually reachable, and the walk would
// pick a spurious "merge" one arm earlier than the real post-dominator (which
// then gets wrongly absorbed into an `if` branch, producing infinite loops).
// The block where the try body and the catch body reconverge: the single
// block control falls into when it leaves the protected range by a normal
// edge, provided the catch reaches it too. A block only the try flows into
// (a `return` the compiler left just outside the range) is the tail of the
// try body and stays there.
//
// Inside a loop, an exit that leaves the loop or goes back to its header is a
// `break`/`continue`, not a join, and is ignored. When several exits remain
// (Hermes copies a `finally` body onto every way out of the try, so each exit
// is its own copy of it), the join is the nearest block the catch reaches that
// every remaining exit also reaches, with all walks kept inside the loop.
fn try_region_exit(
    cfg: &CFG,
    region: &HashSet<BlockId>,
    catch: BlockId,
    innermost_loop: Option<&LoopInfo>,
) -> Option<BlockId> {
    let in_scope = |b: BlockId| -> bool {
        innermost_loop.is_none_or(|l| b != l.header && l.body.contains(&b))
    };
    let mut exits: Vec<BlockId> = Vec::new();
    for &b in region {
        let Some(block) = cfg.get(b) else { continue };
        for succ in block.successors() {
            if succ != catch && !region.contains(&succ) && !exits.contains(&succ) {
                exits.push(succ);
            }
        }
    }
    exits.retain(|&e| in_scope(e));
    // Every block reachable from `from` without leaving the loop.
    let reach = |from: BlockId| -> HashSet<BlockId> {
        let mut seen: HashSet<BlockId> = HashSet::new();
        let mut stack = vec![from];
        while let Some(b) = stack.pop() {
            if !in_scope(b) || !seen.insert(b) {
                continue;
            }
            if let Some(block) = cfg.get(b) {
                stack.extend(block.successors());
            }
        }
        seen
    };
    // A catch that can only end by throwing (the catch-all that runs a
    // `finally` body and rethrows) never rejoins the try. The code after the
    // protected range then follows the whole statement, so it goes after it:
    // kept inside the try, it fell under this handler and ran the finally body
    // a second time when it threw.
    let catch_reach = reach(catch);
    let catch_always_throws = !catch_reach.is_empty()
        && catch_reach.iter().all(|&b| {
            cfg.get(b).is_some_and(|block| {
                !matches!(block.terminator, Terminator::Return(_) | Terminator::None)
                    && block.successors().iter().all(|&s| in_scope(s))
            })
        });
    match exits.as_slice() {
        [] => None,
        [only] if catch_reach.contains(only) || catch_always_throws => Some(*only),
        _ => {
            let exit_reach: Vec<HashSet<BlockId>> = exits.iter().map(|&e| reach(e)).collect();
            let mut seen: HashSet<BlockId> = HashSet::new();
            let mut queue = std::collections::VecDeque::from([catch]);
            while let Some(b) = queue.pop_front() {
                if !in_scope(b) || !seen.insert(b) {
                    continue;
                }
                if b != catch && exit_reach.iter().all(|r| r.contains(&b)) {
                    return Some(b);
                }
                if let Some(block) = cfg.get(b) {
                    queue.extend(block.successors());
                }
            }
            None
        }
    }
}

pub(super) fn find_merge_point(
    cfg: &CFG,
    header: BlockId,
    a: BlockId,
    b: BlockId,
) -> Option<BlockId> {
    use std::collections::VecDeque;
    // All blocks reachable from `a` without passing back through `header`.
    let mut reach_a: HashSet<BlockId> = HashSet::new();
    let mut stack = vec![a];
    while let Some(n) = stack.pop() {
        if n == header || !reach_a.insert(n) {
            continue;
        }
        if let Some(blk) = cfg.get(n) {
            for s in blk.successors() {
                stack.push(s);
            }
        }
    }
    // Breadth-first from `b` for the NEAREST block also reachable from `a`,
    // likewise never crossing back through `header`.
    let mut seen: HashSet<BlockId> = HashSet::new();
    let mut queue: VecDeque<BlockId> = VecDeque::new();
    queue.push_back(b);
    while let Some(n) = queue.pop_front() {
        if n == header || !seen.insert(n) {
            continue;
        }
        if reach_a.contains(&n) {
            return Some(n);
        }
        if let Some(blk) = cfg.get(n) {
            for s in blk.successors() {
                queue.push_back(s);
            }
        }
    }
    None
}
