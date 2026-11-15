function pick(o, c) { var v = o.load(); if (c) { print("first"); print(v); } else { print("none"); } return 1; }
function other(o, c) { var v = o.load(); if (c) { print("first"); } else { print("second"); print(v); } return 2; }
var src = { n: 0, load: function () { this.n++; return "v" + this.n; } };
print(pick(src, true), pick(src, false), other(src, true), other(src, false));
