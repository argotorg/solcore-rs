import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.ABIGeneric;

// Each level branches into distinct Box/Wrap substitutions. Level7 and the
// Pad arguments create a finite, balanced ABI evidence graph exceeding the
// inspection budget; the late manual decoder must never be silently skipped.
enum Box<a> { Box(a) }
enum Wrap<a> { Wrap(a) }
enum Level0<a> { Level0(a) }
enum Level1<a> { Level1(Level0<Box<a>>, Level0<Wrap<a>>) }
enum Level2<a> { Level2(Level1<Box<a>>, Level1<Wrap<a>>) }
enum Level3<a> { Level3(Level2<Box<a>>, Level2<Wrap<a>>) }
enum Level4<a> { Level4(Level3<Box<a>>, Level3<Wrap<a>>) }
enum Level5<a> { Level5(Level4<Box<a>>, Level4<Wrap<a>>) }
enum Level6<a> { Level6(Level5<Box<a>>, Level5<Wrap<a>>) }
enum Level7<a> { Level7(Level6<Box<a>>, Level6<Wrap<a>>) }
enum Pad0 { Pad0(uint256) }
enum Pad1 { Pad1(uint256) }
enum Pad2 { Pad2(uint256) }
enum Pad3 { Pad3(uint256) }
enum Pad4 { Pad4(uint256) }
enum Pad5 { Pad5(uint256) }
enum Pad6 { Pad6(uint256) }
enum Pad7 { Pad7(uint256) }
enum Pad8 { Pad8(uint256) }
enum Late { Late(word) }

impl ABIDecode<ABIDecoder<word, MemoryWordReader>, word> {
  function decode(d: ABIDecoder<word, MemoryWordReader>, offset: word) returns (word) {
    return 42;
  }
}

contract C {
  constructor(
    first: Level7<uint256>,
    pad0: Pad0,
    pad1: Pad1,
    pad2: Pad2,
    pad3: Pad3,
    pad4: Pad4,
    pad5: Pad5,
    pad6: Pad6,
    pad7: Pad7,
    pad8: Pad8,
    late: Late
  ) {}
}
