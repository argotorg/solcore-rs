// Entry point whose type never becomes ground: `main` is polymorphic and is
// the specialization root (no contract), so ensure_closed fails with
// context "entry specialization".  Judge the phrasing of that message.

forall a . function main(x : a) -> a {
  return x;
}
