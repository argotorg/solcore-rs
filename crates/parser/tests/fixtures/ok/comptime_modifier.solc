type comptime = word;

contract ComptimeModifier {
  function f(comptime x : word) -> comptime word {
    return x;
  }

  function identifier(x : comptime) -> comptime {
    let comptime : word = 1;
    let y : comptime word = f(comptime);
    return y;
  }
}
