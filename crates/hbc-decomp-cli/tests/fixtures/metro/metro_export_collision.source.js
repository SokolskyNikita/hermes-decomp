// Metro __d/__r shape, compiled with npm hermes-compiler@250829098.0.16 (HBC 98) -O.
// Module 0 reads `useTheme` off a dependency into a local and also exports a
// function named `useTheme` that calls that local, the shape Babel emits for
// `const { useTheme: useAppTheme } = dep; export function useTheme(o) { ... }`.
// Module 2 is the entry and prints through both exports.
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
  Object.defineProperty(exports, "__esModule", { value: true });
  exports.useTheme = useTheme;
  exports.useInternalTheme = void 0;
  var theming = _$$_REQUIRE(_dependencyMap[0]);
  var useAppTheme = theming.useTheme;
  function useTheme(o) { return useAppTheme(o); }
  var useInternalTheme = function (o) { return useAppTheme(o) + "!"; };
  exports.useInternalTheme = useInternalTheme;
}, 0, [1]);
__d(function (global, _$$_REQUIRE, _$$_IMPORT_DEFAULT, _$$_IMPORT_ALL, module, exports, _dependencyMap) {
  "use strict";
  exports.useTheme = function (o) { return "theme:" + o; };
}, 1, []);
__d(function (global, _$$_REQUIRE, _$$_IMPORT_DEFAULT, _$$_IMPORT_ALL, module, exports, _dependencyMap) {
  "use strict";
  var theme = _$$_REQUIRE(_dependencyMap[0]);
  print(theme.useTheme("dark"));
  print(theme.useInternalTheme("light"));
}, 2, [0]);
__r(2);
