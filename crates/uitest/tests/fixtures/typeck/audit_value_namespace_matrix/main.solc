import util as U;

data Opt = Some(word) | None;
type Alias = word;
contract K { function main() -> word { return 0; } }
forall a . class a:C {}

function adt_value() -> word {
  return Opt;
}

function alias_value() -> word {
  return Alias;
}

function contract_value() -> word {
  return K;
}

function class_value() -> word {
  return C;
}

function builtin_type_value() -> word {
  return word;
}

function builtin_class_value() -> word {
  return Int;
}

forall a . function type_var_value() -> word {
  return a;
}

function module_value() -> word {
  return U;
}

function type_as_callee() -> word {
  return Opt();
}

function module_as_callee() -> word {
  return U();
}

function type_in_binop() -> word {
  return Opt + 1;
}
