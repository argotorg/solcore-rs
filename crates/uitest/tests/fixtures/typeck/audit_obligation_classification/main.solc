function literal_as_callee() returns (word) {
  return 1();
}

function word_as_callee() returns (word) {
  let x: word;
  return x();
}

function from_integer_bad_arg() returns (word) {
  return Int.fromInteger(true);
}

function open_invokable<a>(x: a) returns (word) {
  return invoke(x, ());
}
