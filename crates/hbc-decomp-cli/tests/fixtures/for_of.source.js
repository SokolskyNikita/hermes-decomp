// for-of at -O: plain loop, break, early return (IteratorClose), return
// inside the loop body, [k, v] destructuring in the loop, plain array
// destructuring. Shapes from a production RN bundle (getCapabilityValue, crash-log
// redaction over Object.entries).
function getCap(obj, path) {
  if (!obj) return "none";
  var cur = obj;
  for (const p of path.split(".")) {
    if (cur === undefined || cur === null) return "missing";
    cur = cur[p];
  }
  return cur;
}
function upTo(xs, stop) {
  let t = 0;
  for (const x of xs) {
    if (x === stop) break;
    t += x;
  }
  return t;
}
function sumAll(xs) {
  let t = 0;
  for (const x of xs) t += x;
  return t;
}
function findFirst(xs, k) {
  for (const x of xs) {
    if (x.id === k) return x.id;
  }
  return -1;
}
function firstNeg(xs) {
  let n = 0;
  for (const x of xs) {
    if (x < 0) { print("neg"); return; }
    n++;
  }
  print("count " + n);
}
function redact(o) {
  const out = {};
  for (const [k, v] of Object.entries(o)) {
    out[k] = k.length > 3 ? v : "[R]";
  }
  return out;
}
function pair(p) {
  const [a, b] = p;
  return a + b;
}
print(getCap({a: {b: 3}}, "a.b"), getCap({a: null}, "a.b.c"), getCap(0, "a"));
print(upTo([1, 2, 3, 4], 3), sumAll(new Set([1, 2, 3])), findFirst([{id: 1}, {id: 2}], 2), findFirst([], 1));
firstNeg([1, -1, 2]);
firstNeg([1, 2]);
print(JSON.stringify(redact({ab: 1, abcd: 2})), pair([4, 5]));
