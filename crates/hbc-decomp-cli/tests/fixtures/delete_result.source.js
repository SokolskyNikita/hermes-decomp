// The value of `delete` is a boolean the program can read.
function f(o, arr) {
  var r1 = delete o.a, r2 = delete o["b"], r3 = delete o.c.d;
  delete arr[1];
  var k = "body";
  var r4 = delete o[k];
  print(r1, r2, r3, r4, JSON.stringify(o), JSON.stringify(arr));
}
f({ a: 1, b: 2, c: { d: 3 }, body: 4 }, [1, 2, 3]);
