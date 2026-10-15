import * from std;
import * from std.dispatch;
import * from std.Generic;
import * from std.StorageGeneric;

// An enumeration with more than two constructors.
//
// The SOP representation is a RIGHT-NESTED sum, so `Color` becomes
// `sum((), sum((), ()))` and the constructors are encoded as
//
//   Red   = inl(())            tag 0 at slot p
//   Green = inr(inl(()))       tag 1 at slot p, tag 0 at slot p+1
//   Blue  = inr(inr(()))       tag 1 at slot p, tag 1 at slot p+1
//
// so the tag is spread unary-style across the nesting levels and the type
// occupies 1 + max(0, 1 + max(0, 0)) = 2 slots.
//
// `Green` is the only constructor that exercises the `inr(inl(...))` path,
// which is exactly the sum nesting that CanStore.load has to reconstruct.
enum Color { Red, Green, Blue }

// A three-constructor sum whose branches carry payloads of different widths.
// rep = sum(uint256, sum((uint256, uint256), ())), so
//   size = 1 + max(1, 1 + max(2, 0)) = 4.
enum Shape { Dot(uint256), Seg(uint256, uint256), Nothing }

// #[scenario("storage_adt_enum")]
// constructor() (asserts size(Color)=2, size(Shape)=4; sets Red / Nothing)
// #[deploy(0x) -> success]
// #[raw(0x51f91066, label = "tag() -> 0 (Red, set by the constructor)") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0x99af9df4, label = "shapeTag() -> 2 (Nothing, the last constructor: inr(inr(())))") -> success(0x0000000000000000000000000000000000000000000000000000000000000002)]
// #[raw(0xc2cc8844, label = "setGreen() stores inr(inl(())) — the nested-tag branch") -> success(0x)]
// #[raw(0x51f91066, label = "tag() -> 1 (Green round-trips through the nested sum)") -> success(0x0000000000000000000000000000000000000000000000000000000000000001)]
// #[raw(0x75056e81, label = "setBlue() stores inr(inr(()))") -> success(0x)]
// #[raw(0x51f91066, label = "tag() -> 2 (Blue)") -> success(0x0000000000000000000000000000000000000000000000000000000000000002)]
// #[raw(0x5f210aa8, label = "setRed() overwrites Blue with inl(()) — the inner tag slot is now stale") -> success(0x)]
// #[raw(0x51f91066, label = "tag() -> 0 (the outer tag alone must decide; stale inner slot ignored)") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0xbf0bed0900000000000000000000000000000000000000000000000000000000000000070000000000000000000000000000000000000000000000000000000000000005, label = "setSeg(7, 5) stores inr(inl((7,5))) into the second field") -> success(0x)]
// #[raw(0x99af9df4, label = "shapeTag() -> 1 (Seg)") -> success(0x0000000000000000000000000000000000000000000000000000000000000001)]
// #[raw(0xaa8dd570, label = "shapeSum() -> 12") -> success(0x000000000000000000000000000000000000000000000000000000000000000c)]
// #[raw(0x51f91066, label = "tag() -> 0 still (writing `shape` must not disturb `color`'s slots)") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0x8aea25090000000000000000000000000000000000000000000000000000000000000009, label = "setDot(9) overwrites the wider Seg branch with a narrower one") -> success(0x)]
// #[raw(0x99af9df4, label = "shapeTag() -> 0 (Dot)") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0xaa8dd570, label = "shapeSum() -> 9 (the stale Seg payload in the trailing slots must not leak)") -> success(0x0000000000000000000000000000000000000000000000000000000000000009)]
// #[raw(0xf488c81e, label = "setNothing() stores the zero-slot branch over a payload-carrying one") -> success(0x)]
// #[raw(0x99af9df4, label = "shapeTag() -> 2 (Nothing)") -> success(0x0000000000000000000000000000000000000000000000000000000000000002)]
// #[raw(0xaa8dd570, label = "shapeSum() -> 0") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
contract C {
    color : Color;
    shape : Shape;

    constructor() {
        color = Color.Red;
        shape = Shape.Nothing;
        // 1 tag + max(size (), 1 tag + max(size (), size ())) = 1 + 1 + 0 = 2
        assert(StorageSize.size(@Color) == 2);
        // 1 tag + max(size uint256, 1 tag + max(size (uint256,uint256), size ())) = 1 + 1 + 2 = 4
        assert(StorageSize.size(@Shape) == 4);
    }

    function setRed() public {
        color = Color.Red;
    }

    // inr(inl(())) — the nested-tag branch.
    function setGreen() public {
        color = Color.Green;
    }

    function setBlue() public {
        color = Color.Blue;
    }

    function tag() public returns (uint256) {
        match (color) {
case Color.Red {
return uint256(0);
}
case Color.Green {
return uint256(1);
}
case Color.Blue {
return uint256(2);
}
}
    }

    function setDot(a: uint256) public {
        shape = Shape.Dot(a);
    }

    // inr(inl(...)) again, this time with a product payload.
    function setSeg(a: uint256, b: uint256) public {
        shape = Shape.Seg(a, b);
    }

    function setNothing() public {
        shape = Shape.Nothing;
    }

    function shapeSum() public returns (uint256) {
        match (shape) {
case Shape.Dot(a) {
return a;
}
case Shape.Seg(a, b) {
return a + b;
}
case Shape.Nothing {
return uint256(0);
}
}
    }

    function shapeTag() public returns (uint256) {
        match (shape) {
case Shape.Dot(_) {
return uint256(0);
}
case Shape.Seg(_, _) {
return uint256(1);
}
case Shape.Nothing {
return uint256(2);
}
}
    }
}
