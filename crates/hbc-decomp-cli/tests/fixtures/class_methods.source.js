// An ES5 class built inside an IIFE (TypeScript / Babel loose output). The
// IIFE is not a module factory, so the class is rendered with the inline
// bodies of the first pass; the callbacks inside its methods must still be
// filled in (HBC 84 printed `/* body not rendered */` for each).
var A = (function () {
  function A() { this.m = {}; this.m.x = new B(); }
  function B() {}
  B.prototype.configure = function (c) { print("cfg " + c); };
  A.prototype.configure = function (c) {
    var self = this;
    Object.keys(this.m).forEach(function (k) { self.m[k].configure(c); });
    return Object.keys(this.m).map(function (k) { return k + "!"; });
  };
  return A;
})();
print(new A().configure(1));
