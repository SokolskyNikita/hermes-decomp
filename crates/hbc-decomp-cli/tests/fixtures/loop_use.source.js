// A value read more than once, once inside a loop body: the multi-use inliner
// dropped `t = o.traits.err` and left `t` undeclared in the loop.
function enc(o, keys) {
  var t = o.traits.err;
  var r = {};
  for (var i = 0; i < keys.length; i++) r[keys[i]] = t + keys[i];
  return JSON.stringify(r) + t;
}
function count(o, n) {
  var lim = o.cfg.limit;
  var hits = 0;
  for (var i = 0; i < n; i++) {
    if (i < lim) hits++;
    o.cfg.limit = lim - 1;
  }
  return hits + ":" + lim;
}
var o = { traits: { err: "E" }, cfg: { limit: 2 } };
print(enc(o, ["x", "y"]), count(o, 4));
