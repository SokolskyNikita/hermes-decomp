use crate::ir::{AssignTarget, Binding, BlockId, Statement, CFG};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DefSite {
    pub block: BlockId,
    pub stmt_index: usize,
    pub register: u32,
}

// Reaching Definitions analysis determines which assignments (definitions) might reach a given point.
// A definition `d` reaches point `p` if there is a path from `d` to `p` where `d` is not "killed" (overwritten).
//
// Use case: Constant Propagation, Copy Propagation.
// If only one definition reaches a use, and that definition is a constant, we can inline it.
#[derive(Debug)]
pub struct ReachingDefs {
    pub reaching_in: BTreeMap<BlockId, HashSet<DefSite>>,
    pub reaching_out: BTreeMap<BlockId, HashSet<DefSite>>,
}

impl ReachingDefs {
    // Same equations and result as the textbook round-robin version, solved on
    // dense bitsets over a numbering of the definition sites. The earlier code
    // rebuilt gen/kill and scanned every block for predecessors on each visit,
    // with hashed sets throughout, which made one large function (a 20 KB body
    // or a 160 KB Metro factory) take seconds.
    //
    // IN[B]  = U OUT[P] for P in predecessors(B), plus IN and OUT of every block
    //          an exception edge into B comes from
    // OUT[B] = GEN[B] U (IN[B] - KILL[B]), where KILL[B] is every definition of a
    //          register B assigns that was made in another block
    //
    // Blocks not in the reverse postorder (unreachable) keep IN = {} and
    // OUT = GEN, as before. Both sides are monotone, so iterating to the fixed
    // point in any order gives the same (least) solution.
    pub fn analyze(cfg: &CFG) -> Self {
        let ids: Vec<BlockId> = cfg.block_ids().collect();
        let nb = ids.len();
        let index: HashMap<BlockId, usize> = ids.iter().enumerate().map(|(i, &b)| (b, i)).collect();

        // Number every register definition site; gen = last def per register.
        let mut sites: Vec<DefSite> = Vec::new();
        let mut gen_sites: Vec<Vec<usize>> = Vec::with_capacity(nb);
        let mut assigned: Vec<Vec<u32>> = Vec::with_capacity(nb);
        let mut defs_of_reg: HashMap<u32, Vec<usize>> = HashMap::new();
        for &id in &ids {
            let mut last: BTreeMap<u32, usize> = BTreeMap::new();
            if let Some(block) = cfg.get(id) {
                for (i, stmt) in block.statements.iter().enumerate() {
                    if let Statement::Assign {
                        target: AssignTarget::Binding(Binding::Register(r)),
                        ..
                    } = stmt
                    {
                        let d = sites.len();
                        sites.push(DefSite {
                            block: id,
                            stmt_index: i,
                            register: *r,
                        });
                        defs_of_reg.entry(*r).or_default().push(d);
                        last.insert(*r, d);
                    }
                }
            }
            assigned.push(last.keys().copied().collect());
            gen_sites.push(last.into_values().collect());
        }
        let nd = sites.len();
        let words = nd.div_ceil(64);

        // Per register: bitset of its definition sites.
        let reg_mask: HashMap<u32, Vec<u64>> = defs_of_reg
            .iter()
            .map(|(&r, ds)| {
                let mut m = vec![0u64; words];
                for &d in ds {
                    m[d / 64] |= 1 << (d % 64);
                }
                (r, m)
            })
            .collect();
        // Per block: KILL (defs of its assigned registers from other blocks) and GEN.
        let mut keep_mask: Vec<Vec<u64>> = Vec::with_capacity(nb);
        let mut gen_mask: Vec<Vec<u64>> = Vec::with_capacity(nb);
        for b in 0..nb {
            let mut kill = vec![0u64; words];
            for r in &assigned[b] {
                for (k, m) in kill.iter_mut().zip(&reg_mask[r]) {
                    *k |= *m;
                }
            }
            for r in &assigned[b] {
                for &d in &defs_of_reg[r] {
                    if sites[d].block == ids[b] {
                        kill[d / 64] &= !(1 << (d % 64));
                    }
                }
            }
            for k in kill.iter_mut() {
                *k = !*k;
            }
            keep_mask.push(kill);
            let mut g = vec![0u64; words];
            for &d in &gen_sites[b] {
                g[d / 64] |= 1 << (d % 64);
            }
            gen_mask.push(g);
        }
        drop(reg_mask);

        // Predecessors (all blocks, reachable or not) and exception-edge sources.
        let mut preds: Vec<Vec<usize>> = vec![Vec::new(); nb];
        for (b, &id) in ids.iter().enumerate() {
            if let Some(block) = cfg.get(id) {
                let mut seen: Vec<usize> = Vec::new();
                for s in block.successors() {
                    if let Some(&t) = index.get(&s) {
                        if !seen.contains(&t) {
                            seen.push(t);
                            preds[t].push(b);
                        }
                    }
                }
            }
        }
        let exc: Vec<Vec<usize>> = ids
            .iter()
            .map(|&id| {
                cfg.exception_edge_sources(id)
                    .into_iter()
                    .filter_map(|s| index.get(&s).copied())
                    .collect()
            })
            .collect();

        let mut ins: Vec<Vec<u64>> = vec![vec![0u64; words]; nb];
        let mut outs: Vec<Vec<u64>> = gen_mask.clone();

        let rpo: Vec<usize> = cfg
            .reverse_postorder()
            .into_iter()
            .filter_map(|b| index.get(&b).copied())
            .collect();
        let mut new_in = vec![0u64; words];
        let mut new_out = vec![0u64; words];
        let mut changed = true;
        while changed {
            changed = false;
            for &b in &rpo {
                new_in.iter_mut().for_each(|w| *w = 0);
                for &p in &preds[b] {
                    for (w, o) in new_in.iter_mut().zip(&outs[p]) {
                        *w |= *o;
                    }
                }
                for &s in &exc[b] {
                    for ((w, i), o) in new_in.iter_mut().zip(&ins[s]).zip(&outs[s]) {
                        *w |= *i | *o;
                    }
                }
                for (((o, i), k), g) in new_out
                    .iter_mut()
                    .zip(&new_in)
                    .zip(&keep_mask[b])
                    .zip(&gen_mask[b])
                {
                    *o = (*i & *k) | *g;
                }
                if new_in != ins[b] {
                    changed = true;
                    ins[b].copy_from_slice(&new_in);
                }
                if new_out != outs[b] {
                    changed = true;
                    outs[b].copy_from_slice(&new_out);
                }
            }
        }

        let to_set = |bits: &[u64]| -> HashSet<DefSite> {
            let mut set = HashSet::new();
            for (w, &word) in bits.iter().enumerate() {
                let mut word = word;
                while word != 0 {
                    let t = word.trailing_zeros() as usize;
                    set.insert(sites[w * 64 + t]);
                    word &= word - 1;
                }
            }
            set
        };
        let mut reaching_in = BTreeMap::new();
        let mut reaching_out = BTreeMap::new();
        for (b, &id) in ids.iter().enumerate() {
            reaching_in.insert(id, to_set(&ins[b]));
            reaching_out.insert(id, to_set(&outs[b]));
        }

        ReachingDefs {
            reaching_in,
            reaching_out,
        }
    }

    // The original round-robin solver, kept to check `analyze` against.
    #[cfg(test)]
    pub(crate) fn analyze_reference(cfg: &CFG) -> Self {
        let mut reaching_in: BTreeMap<BlockId, HashSet<DefSite>> = BTreeMap::new();
        let mut reaching_out: BTreeMap<BlockId, HashSet<DefSite>> = BTreeMap::new();

        for id in cfg.block_ids() {
            reaching_in.insert(id, HashSet::new());
            reaching_out.insert(id, compute_gen(cfg, id));
        }

        // Fixed-point iteration using Reverse Postorder (RPO).
        // IN[B] = U(OUT[P]) for P in predecessors(B)
        // OUT[B] = GEN[B] U (IN[B] - KILL[B])
        // Forward analysis (unlike liveness which is backward).
        let rpo = cfg.reverse_postorder();
        let mut changed = true;

        while changed {
            changed = false;

            for &block_id in &rpo {
                let mut new_in: HashSet<DefSite> = HashSet::new();
                for pred in cfg.predecessors(block_id) {
                    if let Some(pred_out) = reaching_out.get(&pred) {
                        new_in.extend(pred_out);
                    }
                }
                // A catch block is entered from any point of its protected
                // range, before or after each definition made there. Every
                // definition live on entry to a protected block, or made by
                // it, may therefore reach the catch.
                for src in cfg.exception_edge_sources(block_id) {
                    if let Some(src_in) = reaching_in.get(&src) {
                        new_in.extend(src_in);
                    }
                    if let Some(src_out) = reaching_out.get(&src) {
                        new_in.extend(src_out);
                    }
                }

                // reaching_out = gen(block) ∪ (reaching_in - kill(block))
                let gen = compute_gen(cfg, block_id);
                let kill = compute_kill(cfg, block_id, &new_in);
                let mut new_out = new_in.clone();
                for k in &kill {
                    new_out.remove(k);
                }
                new_out.extend(&gen);

                if new_in != *reaching_in.get(&block_id).unwrap_or(&HashSet::new()) {
                    changed = true;
                    reaching_in.insert(block_id, new_in);
                }
                if new_out != *reaching_out.get(&block_id).unwrap_or(&HashSet::new()) {
                    changed = true;
                    reaching_out.insert(block_id, new_out);
                }
            }
        }

        ReachingDefs {
            reaching_in,
            reaching_out,
        }
    }

    pub fn defs_for(&self, block: BlockId, reg: u32) -> Vec<DefSite> {
        self.reaching_in
            .get(&block)
            .map(|s| s.iter().filter(|d| d.register == reg).copied().collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
fn compute_gen(cfg: &CFG, block_id: BlockId) -> HashSet<DefSite> {
    let mut gen = HashSet::new();
    if let Some(block) = cfg.get(block_id) {
        for (i, stmt) in block.statements.iter().enumerate() {
            if let Statement::Assign {
                target: AssignTarget::Binding(Binding::Register(r)),
                ..
            } = stmt
            {
                // Remove any earlier def of same register in this block
                gen.retain(|d: &DefSite| d.register != *r);
                gen.insert(DefSite {
                    block: block_id,
                    stmt_index: i,
                    register: *r,
                });
            }
        }
    }
    gen
}

#[cfg(test)]
fn compute_kill(cfg: &CFG, block_id: BlockId, reaching: &HashSet<DefSite>) -> HashSet<DefSite> {
    let mut kill = HashSet::new();
    if let Some(block) = cfg.get(block_id) {
        for stmt in &block.statements {
            if let Statement::Assign {
                target: AssignTarget::Binding(Binding::Register(r)),
                ..
            } = stmt
            {
                // Kill all reaching defs of this register from other blocks
                for def in reaching {
                    if def.register == *r && def.block != block_id {
                        kill.insert(*def);
                    }
                }
            }
        }
    }
    kill
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{CFGBuilder, Constant, Expression, Value};

    #[test]
    fn test_reaching_defs() {
        let mut builder = CFGBuilder::new();
        builder.emit(Statement::assign_reg(
            0,
            Expression::constant(Constant::Integer(1)),
        ));
        builder.emit_return(Some(Expression::Value(Value::Binding(Binding::Register(
            0,
        )))));

        let cfg = builder.finish();
        let reaching = ReachingDefs::analyze(&cfg);

        // At entry, no definitions should reach
        assert!(reaching.defs_for(cfg.entry, 0).is_empty());
    }

    // Random CFGs (loops, unreachable blocks, try ranges) must give exactly the
    // sets the original round-robin solver gives.
    #[test]
    fn bitset_solver_matches_reference() {
        let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = |n: u64| -> u64 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        for _case in 0..300 {
            let n_blocks = 1 + next(40) as usize;
            let n_regs = 1 + next(6) as u32;
            let mut builder = CFGBuilder::new();
            let mut blocks = vec![builder.current_block()];
            for _ in 1..n_blocks {
                blocks.push(builder.create_block());
            }
            for (i, &b) in blocks.iter().enumerate() {
                builder.set_current_block(b);
                for _ in 0..next(5) {
                    builder.emit(Statement::assign_reg(
                        next(n_regs as u64) as u32,
                        Expression::constant(Constant::Integer(i as i32)),
                    ));
                }
                match next(4) {
                    0 => builder.emit_return(None),
                    1 => builder.emit_jump(blocks[next(n_blocks as u64) as usize]),
                    _ => builder.emit_branch(
                        Expression::Value(Value::Binding(Binding::Register(0))),
                        blocks[next(n_blocks as u64) as usize],
                        blocks[next(n_blocks as u64) as usize],
                    ),
                }
            }
            let mut cfg = builder.finish();
            if n_blocks > 2 && next(2) == 0 {
                let catch_block = blocks[next(n_blocks as u64) as usize];
                let try_blocks: Vec<BlockId> =
                    (0..1 + next(3)).map(|_| blocks[next(n_blocks as u64) as usize]).collect();
                cfg.exception_handlers.push(crate::ir::CfgExceptionHandler {
                    try_block_start: try_blocks[0],
                    catch_block,
                    try_blocks,
                });
            }
            let fast = ReachingDefs::analyze(&cfg);
            let slow = ReachingDefs::analyze_reference(&cfg);
            assert_eq!(fast.reaching_in, slow.reaching_in);
            assert_eq!(fast.reaching_out, slow.reaching_out);
        }
    }
}
