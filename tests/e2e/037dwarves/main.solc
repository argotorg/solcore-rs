import std.{*};
import std.dispatch.{*};

contract Dwarves {
  data Dwarf = Doc | Grumpy | Sleepy | Bashful | Happy | Sneezy | Dopey;


  function fromEnum(c : Dwarf) -> word {
    match c {
      | Dwarf.Doc      => return 1;
      | Dwarf.Grumpy   => return 2;
      | Dwarf.Sleepy   => return 3;
      | Dwarf.Bashful  => return 4;
      | Dwarf.Happy    => return 5;
      | _              => return 0;
    }
  }

  // #[() -> 5]
  public function run() -> uint256 { return uint256(fromEnum(Dwarf.Happy)); }
}
