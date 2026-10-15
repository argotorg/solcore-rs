import * from std;
import { memory, string, uint256 } from std;
import * from std.dispatch;
pragma no-patterson-condition;
pragma no-coverage-condition;
pragma no-bounded-variable-condition;

// End-to-end test of comptime string materialization into memory(string):
// each form must ABI-encode to the same "Hello, world!" return value.
// #[scenario("stringlit")]
// #[deploy(0x) -> success]
// #[raw(0xef690cc0, label = "greeting()(string)") -> success(0x0000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000000000d48656c6c6f2c20776f726c642100000000000000000000000000000000000000)]
// #[raw(0x84b0d9a0, label = "greetLet()(string)") -> success(0x0000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000000000d48656c6c6f2c20776f726c642100000000000000000000000000000000000000)]
contract C {
    constructor() {}

    // terse: concatLit wrapped in Str.fromString by the desugarer
    function greeting() public returns (memory<string>) {
        return concatLit("Hello, ", "world!");
        // fromString inserted automatically when using concatLit
        // later we may have an operator for that e.g. <>
    }

    // A2: via an intermediate string-typed let (dead-let substitution path)
    function greetLet() public returns (memory<string>) {
        let s: string = "Hello, " + "world!";
        return Str.fromString(s);
        // here fromString needs to be inserted manually
    }
}
