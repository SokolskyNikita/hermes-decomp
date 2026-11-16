// Metro __d/__r shape, compiled with npm hermes-compiler@250829098.0.16 (HBC 98) -O -g0.
// Module 0 reads its dependency only inside string concatenations, which
// decompile as template literals: `${clamp.clamp(-5)}`.
var __modules = {};
function __d(factory, id, deps) { __modules[id] = { factory: factory, deps: deps, module: null }; }
function __r(id) {
  var m = __modules[id];
  if (m.module) return m.module.exports;
  m.module = { exports: {} };
  m.factory(globalThis, __r, function (i) { var e = __r(i); return e && e.__esModule ? e.default : e; },
    function (i) { return __r(i); }, m.module, m.module.exports, m.deps);
  return m.module.exports;
}
__d(function (global, _$$_REQUIRE, _$$_IMPORT_DEFAULT, _$$_IMPORT_ALL, module, exports, _dependencyMap) {
  "use strict";
  var clamp = _$$_REQUIRE(_dependencyMap[0]);
  print("lo:" + clamp.clamp(-5) + "," + clamp.isMissing(null));
}, 0, [1]);
__d(function (global, _$$_REQUIRE, _$$_IMPORT_DEFAULT, _$$_IMPORT_ALL, module, exports, _dependencyMap) {
  "use strict";
  exports.clamp = function (n) { return n < 0 ? 0 : n; };
  exports.isMissing = function (v) { return v == null; };
}, 1, []);
__r(0);
