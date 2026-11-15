function cmp(a, b) { var out = []; if (a === b) out.push("seq"); if (a !== b) out.push("sne"); if (a == b) out.push("eq"); if (a != b) out.push("ne"); return out.join(","); }
function guarded(x) { try { if (x) throw new Error("boom"); return "ok"; } catch (e) { return e.message; } }
function lit() { return { a: 1, b: undefined, c: "x" }; }
var pairs = [[1, 1], [1, "1"], [1, 2]];
print(pairs.map(function (p) { return cmp(p[0], p[1]); }).join(" "), guarded(0), guarded(1), JSON.stringify(lit()), "b" in lit());
