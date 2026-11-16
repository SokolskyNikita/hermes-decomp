// HBC 99 put globalThis.Symbol and globalThis.eval in front of the builtin
// table and added setFunctionName, so CallBuiltin indices from v98 are off by
// two (then three) on v99. The table must follow Builtins.def at
// hermes-v260318099.0.4.

use hbc_decomp::opcode::builtins_for_version;

#[test]
fn v99_builtins_follow_builtins_def() {
    let v99 = builtins_for_version(99);
    assert_eq!(v99.len(), 61);
    assert_eq!(v99[0], "globalThis.Symbol");
    assert_eq!(v99[6], "JSON.stringify");
    assert_eq!(v99[55], "HermesBuiltin.setFunctionName");
    assert_eq!(v99[57], "HermesBuiltin.functionPrototypeCall");
}
