// Loops whose header ends in a jump-table switch (UIntSwitchImm), with a
// try/catch in one case, a throwing default, and exits through both a
// `break` and the loop test. Shape of React's
// commitBeforeMutationEffects_complete: the loop body was dropped.
function walk(root, stopAt) {
  var log = [];
  var next = root;
  while (next !== null) {
    var node = next;
    var flags = node.flags;
    switch (node.tag) {
      case 0: log.push("a" + node.id); break;
      case 1:
        if (flags & 1) {
          try {
            if (node.bad) throw "e" + node.id;
            log.push("b" + node.id);
          } catch (e) {
            log.push("c" + e);
          }
        }
        break;
      case 2: log.push("x" + node.id); break;
      case 3: log.push("y"); break;
      case 4: log.push("z"); break;
      case 5: log.push("w"); break;
      case 6: log.push("v"); break;
      case 7: log.push("u"); break;
      case 8: log.push("q8"); break;
      case 9: log.push("q9"); break;
      case 10: log.push("q10"); break;
      case 11: log.push("q11"); break;
      case 12: log.push("q12"); break;
      case 13: log.push("q13"); break;
      case 14: log.push("q14"); break;
      case 15: log.push("q15"); break;
      default:
        if (flags & 2) throw "unknown";
    }
    if (node.id === stopAt) {
      log.push("stop");
      break;
    }
    next = node.sibling;
  }
  return log.join(",");
}
function count(list) {
  var n = 0;
  var cur = list;
  while (cur !== null) {
    switch (cur.k) {
      case 0: n += 1; break;
      case 1:
        try { if (cur.t) throw 1; n += 10; } catch (e) { n += 100; }
        break;
      case 2: n += 1000; break;
      case 3: n += 2; break;
      case 4: n += 3; break;
      case 5: n += 4; break;
      case 6: n += 5; break;
      case 7: n += 6; break;
      case 8: n += 8; break;
      case 9: n += 9; break;
      case 10: n += 10; break;
      case 11: n += 11; break;
      case 12: n += 12; break;
      case 13: n += 13; break;
      case 14: n += 14; break;
      case 15: n += 15; break;
    }
    cur = cur.next;
  }
  return n;
}
var c = { id: 3, tag: 2, flags: 0, sibling: null };
var b = { id: 2, tag: 1, flags: 1, bad: true, sibling: c };
var a = { id: 1, tag: 1, flags: 1, bad: false, sibling: b };
var r = { id: 0, tag: 0, flags: 0, sibling: a };
print(walk(r, -1));
print(walk(r, 2));
try { walk({ id: 9, tag: 99, flags: 2, sibling: null }, -1); } catch (e) { print("threw " + e); }
var l3 = { k: 2, t: false, next: null };
var l2 = { k: 1, t: true, next: l3 };
var l1 = { k: 1, t: false, next: l2 };
print(count({ k: 0, next: l1 }));
// A loop test that throws: the throw is not the loop's exit (the bound
// check is), and was dropped once a throwing exit stopped being picked.
function allSignals(arr) {
  if (Array.isArray(arr)) {
    for (var i = 0; i < arr.length; i++) {
      if (!(arr[i] instanceof Error)) throw "bad " + i;
    }
  }
  return "ok " + arr.length;
}
print(allSignals([new Error("a"), new Error("b")]));
try { allSignals([new Error("a"), 3]); } catch (e) { print(e); }
