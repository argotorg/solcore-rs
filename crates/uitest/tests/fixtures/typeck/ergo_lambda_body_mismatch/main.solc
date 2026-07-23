function apply(f: function(word) returns (word), x: word) returns (word) {
  return f(x);
}

function g() returns (word) {
  return apply(lam (y: word) { return true; }, 1);
}
