data Option = None | Some(word);

function apply(f: (word) -> Option) -> Option {
  return f(1);
}

function main() -> Option {
  return apply(lam(x) { return .Some(x); });
}
