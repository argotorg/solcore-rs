import std.{*};
import std.dispatch.{*};

data Food = Curry | Beans | Other;
data CFood = Red(Food) | Green(Food) | Nocolor;



  function fromEnum(x : CFood) -> word {
     match x {
       | CFood.Red(Food.Curry) => return 1;
       | CFood.Green(Food.Beans) => return 42;
       | _ => return 3;
     }
  }


contract FoodContract {
  function id(x : CFood) -> CFood {
    return(x);
  }

  // #[() -> 42]
  public function run() -> uint256 {
  return uint256(fromEnum(id(CFood.Green(Food.Beans))));
  }
}
