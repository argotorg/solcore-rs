pragma solcore;

import * from std;
import * from std.dispatch;

// Regression test for the backend, independent of the storage/Generic work:
// matching a sum constructor whose payload is a product of arity >= 3.
//
// On `match`, the scrutinee's location is flattened. Binding the constructor
// payload as a flat slot sequence breaks destructuring of the inner product,
// because taking the first component of a >2-element sequence is ill-typed.
// (A 2-field payload happens to work, since a flat 2-seq is a valid pair.)
//
// No storage and no Generic derivation involved — just constructing and matching
// an ordinary algebraic data type.

enum Shape {
    Dot,
    Tri(uint256, uint256, uint256)
}

// #[scenario("sum_wide_product")]
// constructor()
// #[deploy(0x) -> success]
// #[raw(0xac094806000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000000003, label = "triSum(1,2,3) -> 6") -> success(0x0000000000000000000000000000000000000000000000000000000000000006)]
// #[raw(0xac094806000000000000000000000000000000000000000000000000000000000000006400000000000000000000000000000000000000000000000000000000000000c8000000000000000000000000000000000000000000000000000000000000012c, label = "triSum(100,200,300) -> 600") -> success(0x0000000000000000000000000000000000000000000000000000000000000258)]
contract C {
    constructor() {}

    // Build Tri(a,b,c) then match it back out: exercises a sum whose payload is
    // a 3-field product.
    function triSum(a: uint256, b: uint256, c: uint256) public returns (uint256) {
        let s: Shape = Shape.Tri(a, b, c);
        match (s) {
            case Shape.Dot {
                return uint256(0);
            }
            case Shape.Tri(x, y, z) {
                return x + y + z;
            }
        }
    }
}
