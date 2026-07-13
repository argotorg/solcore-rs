function pick(x : word) -> word {
  match x {
  | 0x0A => return 0;
  | 10 => return 1;
  | _ => return 2;
  }
}

function wrapped(x : word) -> word {
  match x {
  | 0 => return 0;
  | 115792089237316195423570985008687907853269984665640564039457584007913129639936 => return 1;
  | _ => return 2;
  }
}

function exact(x : integer) -> word {
  match x {
  | 0 => return 0;
  | 115792089237316195423570985008687907853269984665640564039457584007913129639936 => return 1;
  | _ => return 2;
  }
}
