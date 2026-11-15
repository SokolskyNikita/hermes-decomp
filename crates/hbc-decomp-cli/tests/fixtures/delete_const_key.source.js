// delete obj[constString] inside a try/finally in an async method (state machine inside while/try)
class A { async ser(a, c) { return a; } }
class B extends A {
  async ser(ctx, cfg) {
    await super.ser(ctx, cfg);
    try {
      if (ctx.input === "unit") {
        delete cfg["body"];
        delete cfg.headers["content-type"];
      } else if (!cfg.body) {
        cfg.body = ctx.flush();
      }
      await super.ser(ctx, cfg);
    } finally {
      cfg.done = 1;
    }
    return cfg;
  }
}
new B().ser({ input: "unit" }, { body: 1, headers: { "content-type": "x" } }).then((c) => print(JSON.stringify(c)));
