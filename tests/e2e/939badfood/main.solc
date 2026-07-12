import std.{*};
import std.dispatch.{*};

forall a . class a: Enum {
  function fromEnum(x : a) -> word;
}

data Food = Curry | Beans | Other;

instance Food : Enum {
  function fromEnum(x : Food) -> word {
     match x {
       | Food.Curry => return 1;
       | Food.Beans => return 2;
       | Food.Other => return 3;
     }
  }
}

contract FoodContract {
  // #[() -> 2]
  public function run() -> uint256 {
        return uint256(Enum.fromEnum(Food.Beans));
  }
}
