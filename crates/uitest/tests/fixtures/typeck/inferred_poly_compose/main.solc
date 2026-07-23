contract C {
  function compose(f, g) public {
    return lam (x) {
      return f(g(x));
    };
  }

  function id(x: word) public returns (word) {
    return x;
  }

  function main() public returns (word) {
    let f = compose(id, id);
    return f(42);
  }
}
