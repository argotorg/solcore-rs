pragma solcore;

enum D { C }

function f(x: D) returns (word) {
  match (x) {
case C() {
return 1;
}
}
}
