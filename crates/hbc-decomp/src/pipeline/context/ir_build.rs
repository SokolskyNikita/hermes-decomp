// Metro registry + parallel optimized IR generation.
use super::super::{apply_register_naming, generate_ir, DecompileOptionsV2};
use super::PipelineContext;
use crate::file::BytecodeFile;
use crate::ir::Statement;
use crate::opcode::BytecodeFormat;
use crate::transforms;
use std::collections::BTreeMap;

impl PipelineContext {
    pub(super) fn build_metro_registry(
        file: &BytecodeFile,
        format: &BytecodeFormat,
    ) -> crate::analysis::MetroRegistry {
        let t = std::time::Instant::now();
        let raw_options = DecompileOptionsV2 {
            resolve_strings: true,
            ..DecompileOptionsV2::default()
        };

        let mut registry = crate::analysis::MetroRegistry::new();
        let global_idx = file.header.global_code_index;
        // function_id -> declared parameter count (this-excluded), so Metro
        // factory roles are derived from the real arity (4-param classic vs
        // 7-param modern with importDefault/importAll).
        let param_counts: std::collections::HashMap<u32, u32> = file
            .function_headers
            .iter()
            .enumerate()
            .map(|(id, h)| (id as u32, h.param_count().saturating_sub(1)))
            .collect();
        if let Ok(stmts) = generate_ir(file, format, global_idx, &raw_options, None, false) {
            registry.analyze_statements_with_params(&stmts, &param_counts);
        }
        log::debug!(
            "[pipeline] metro detection: {:.2?} ({} modules)",
            t.elapsed(),
            registry.modules.len()
        );
        registry
    }

    // Phase 2: Generate optimized IR in parallel, then run closure analysis sequentially.
    pub(super) fn generate_all_optimized_ir(
        file: &BytecodeFile,
        format: &BytecodeFormat,
        options: &DecompileOptionsV2,
        closure_ctx: &mut Option<crate::analysis::ClosureContext>,
    ) -> BTreeMap<u32, Vec<Statement>> {
        let t = std::time::Instant::now();
        let named_irs: Vec<Option<(u32, Vec<Statement>)>> = {
            let ctx_ref = closure_ctx.as_ref();
            let sizes: Vec<u32> = (0..file.header.function_count as usize)
                .map(|i| {
                    file.function_headers
                        .get(i)
                        .map_or(0, |h| h.bytecode_size_in_bytes())
                })
                .collect();
            largest_first(&sizes, |i| {
                let stmts = generate_ir(file, format, i, options, ctx_ref, false)
                    .map_err(|e| log::debug!("[pipeline] IR gen failed for func {i}: {e}"))
                    .ok()?;
                let named = apply_register_naming(stmts, file, i);
                let semantic = transforms::infer_variable_names(named);
                let mut final_stmts = semantic;
                crate::transforms::simplify_statements(&mut final_stmts);
                Some((i, final_stmts))
            })
        };
        log::debug!(
            "[pipeline] optimized IR generation (parallel): {:.2?}",
            t.elapsed()
        );

        let t = std::time::Instant::now();
        let mut all_ir = BTreeMap::new();
        for item in named_irs.into_iter().flatten() {
            let (i, final_stmts) = item;
            if let Some(ctx) = closure_ctx.as_mut() {
                ctx.analyze_function(i, &final_stmts);
            }
            all_ir.insert(i, final_stmts);
        }
        if let Some(ctx) = closure_ctx.as_mut() {
            ctx.propagate_async_to_generators();
        }
        log::debug!("[pipeline] closure analyze + insert: {:.2?}", t.elapsed());
        all_ir
    }
}

// Run `f` on every function id and return the results in id order. A few
// functions (large Metro factories) take far longer than the rest, and a plain
// parallel iterator may reach them last, leaving one core busy while the others
// sit idle. Workers instead take ids from a shared queue sorted by bytecode
// size, largest first. The order of execution never reaches the output: each
// result is stored under its own id.
fn largest_first<T: Send>(sizes: &[u32], f: impl Fn(u32) -> Option<T> + Sync) -> Vec<Option<T>> {
    use rayon::prelude::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let n = sizes.len();
    let mut order: Vec<u32> = (0..n as u32).collect();
    order.sort_by_key(|&i| (std::cmp::Reverse(sizes[i as usize]), i));
    let next = AtomicUsize::new(0);
    let workers = rayon::current_num_threads().max(1);
    let done: Vec<Vec<(u32, Option<T>)>> = (0..workers)
        .into_par_iter()
        .with_max_len(1)
        .map(|_| {
            let mut out = Vec::new();
            loop {
                let k = next.fetch_add(1, Ordering::Relaxed);
                let Some(&i) = order.get(k) else { break };
                out.push((i, f(i)));
            }
            out
        })
        .collect();
    let mut slots: Vec<Option<T>> = (0..n).map(|_| None).collect();
    for (i, r) in done.into_iter().flatten() {
        slots[i as usize] = r;
    }
    slots
}

#[cfg(test)]
mod tests {
    use super::largest_first;
    use std::sync::Mutex;

    #[test]
    fn largest_first_starts_big_functions_first_and_keeps_id_order() {
        let sizes = [3, 10, 1, 10, 0];
        let one = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap();
        let started = Mutex::new(Vec::new());
        let out = one.install(|| {
            largest_first(&sizes, |i| {
                started.lock().unwrap().push(i);
                (i != 2).then_some(i * 10)
            })
        });
        assert_eq!(*started.lock().unwrap(), vec![1, 3, 0, 2, 4]);
        assert_eq!(out, vec![Some(0), Some(10), None, Some(30), Some(40)]);

        let four = rayon::ThreadPoolBuilder::new()
            .num_threads(4)
            .build()
            .unwrap();
        let out = four.install(|| largest_first(&sizes, |i| (i != 2).then_some(i * 10)));
        assert_eq!(out, vec![Some(0), Some(10), None, Some(30), Some(40)]);
    }
}
