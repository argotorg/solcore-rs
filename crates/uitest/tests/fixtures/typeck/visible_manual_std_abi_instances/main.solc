import std.{*};
import std.dispatch.{*};

instance word:ABIAttribs {
  function headSize(p: Proxy(word)) -> word { return 32; }
  function isStatic(p: Proxy(word)) -> bool { return true; }
}

instance word:ABIEncode {
  function encodeInto(x: word, base: word, offset: word, tail: word) -> word { return tail; }
}

instance ABIDecoder(word, CalldataWordReader):ABIDecode(word) {
  function decode(d: ABIDecoder(word, CalldataWordReader), offset: word) -> word { return 0; }
}

instance word:SigString {
  function sigStr(p: Proxy(word)) -> string { return "uint256"; }
}

contract C {
  public function echo(value: word) -> word { return value; }
}
