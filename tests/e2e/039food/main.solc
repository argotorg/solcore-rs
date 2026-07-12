
data Food = Curry | Beans | Other;
data CFood = Red(Food) | Green(Food) | Nocolor;




  function fromEnum(x : Food) -> word {
     match x {
       | Food.Curry => return 1;
       | Food.Beans => return 42;
       | Food.Other => return 3;
     }
  }


contract FoodContract {
  function eat(x : CFood) -> Food {
    match x {
       | CFood.Red(f) => return f;
       | CFood.Green(f) => return f;
       | _ => return Food.Other;
    }
  }

  // #[() -> 42]
  public function run() -> uint256 {
  return uint256(fromEnum(eat(CFood.Green(Food.Beans))));
  }
}
import std.{*};
import std.dispatch.{*};
