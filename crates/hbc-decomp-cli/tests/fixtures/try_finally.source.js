// try/catch/finally shapes. The catch and the finally's catch-all handler
// start at the same block; a catch sits in a loop with break and a finally;
// code after an inner try/finally is outside its handler.
function withFinally(x) {
  var log = [];
  try { log.push("t"); if (x) throw x; log.push("t2"); }
  catch (e) { log.push("c" + e); }
  finally { log.push("f"); }
  return log.join(",");
}
function nested(x) {
  var log = [];
  try {
    try { if (x === 1) throw "inner"; log.push("i"); }
    catch (e) { log.push("ci"); if (x === 1) throw "rethrown"; }
    finally { log.push("fi"); }
    if (x === 2) throw "outer";
    log.push("o");
  } catch (e2) { log.push("co(" + e2 + ")"); }
  return log.join(",");
}
function loopCatch(n) {
  var s = 0;
  for (var i = 0; i < n; i++) {
    try { if (i % 3 === 0) throw i; if (i === 7) break; s += i; }
    catch (e) { s += 100; continue; }
    finally { s += 1000; }
  }
  return s;
}
print(withFinally(0)); print(withFinally("X"));
print(nested(0)); print(nested(1)); print(nested(2));
print(loopCatch(10)); print(loopCatch(5));
