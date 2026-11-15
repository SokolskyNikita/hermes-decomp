mod conversion;
mod exceptions;
mod loops;
mod recovery;

use crate::analysis::loops::LoopInfo;
use crate::ir::{BlockId, CfgExceptionHandler, Expression, Statement};
use std::collections::{BTreeMap, HashSet};

// Context passed through recursive structure recovery, avoiding 8-parameter functions.
pub(crate) struct RecoveryCtx<'a> {
    pub cfg: &'a crate::ir::CFG,
    pub loops: &'a [LoopInfo],
    pub visited: &'a mut HashSet<BlockId>,
    // Handlers whose protected range starts at a block, outermost first. Several
    // can share a start: `try { } catch { } finally { }` compiles to a catch
    // handler and a catch-all (finally) handler over the same first block.
    pub try_starts: &'a BTreeMap<BlockId, Vec<&'a CfgExceptionHandler>>,
    // A loop header whose block walk must not start the loop again: set while
    // the body of a loop whose header opens a `try` is recovered.
    pub plain_header: Option<BlockId>,
    // The loop-stack depth of the switch whose cases are being recovered, if
    // any. A `break` out of the innermost loop from inside it needs the loop's
    // label, or it would only leave the switch.
    pub switch_loop_depth: Option<usize>,
}

impl RecoveryCtx<'_> {
    // The label a `break`/`continue` to loop `i` of a stack of `depth` loops
    // needs: an outer loop's, or the innermost loop's when a `break` sits in a
    // switch at that depth.
    pub(super) fn loop_label(&self, i: usize, depth: usize, is_break: bool) -> Option<String> {
        let innermost = i + 1 == depth;
        let in_switch = is_break && self.switch_loop_depth == Some(depth);
        (!innermost || in_switch).then(|| format!("label{i}"))
    }
}

#[derive(Debug, Clone)]
pub enum Structure {
    Block(BlockId, Vec<Statement>),
    Sequence(Vec<Structure>),
    If {
        condition: Expression,
        then_: Box<Structure>,
        else_: Box<Structure>,
    },
    While {
        condition: Expression,
        body: Box<Structure>,
    },
    DoWhile {
        body: Box<Structure>,
        condition: Expression,
    },
    For {
        init: Box<Structure>,
        condition: Expression,
        update: Box<Structure>,
        body: Box<Structure>,
    },
    Switch {
        discriminant: Expression,
        cases: Vec<(Expression, Structure)>,
        default: Box<Structure>,
    },
    TryCatch {
        try_body: Box<Structure>,
        catch_param: Option<String>,
        catch_body: Box<Structure>,
    },
    Return(Option<Expression>),
    Break(Option<String>),
    Continue(Option<String>),
    Label(String, Box<Structure>),
}

pub struct StructureAnalysis {
    pub root: Structure,
    pub loops: Vec<LoopInfo>,
}

impl StructureAnalysis {
    pub fn analyze(cfg: &crate::ir::CFG) -> Self {
        let (root, loops) = recovery::analyze(cfg);
        StructureAnalysis { root, loops }
    }
}
