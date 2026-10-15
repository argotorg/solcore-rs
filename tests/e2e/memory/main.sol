import * from std;
import * from std.dispatch;
import { mstore } from std.opcodes;

// #[scenario("memory")]
// constructor()
// #[deploy(0x) -> success]
// #[raw(0x9abc6cd9, label = "dirty_allocate() -> (bytes)") -> success(0x0000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000000000000deadc0de)]
// #[raw(0xa33a0cf7, label = "clear_allocate() -> (bytes)") -> success(0x000000000000000000000000000000000000000000000000000000000000002000000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000000)]
contract C {
    function dirty_allocate() public returns (memory<bytes>) {
        mstore(get_free_memory() + 32, 0xdeadc0de);
        let ptr = allocate_memory(32 + 32);
        mstore(ptr, 32);
        return memory(ptr);
    }

    function clear_allocate() public returns (memory<bytes>) {
        mstore(get_free_memory() + 32, 0xdeadc0de);
        let ptr = allocate_zeroed_memory(32 + 32);
        mstore(ptr, 32);
        return memory(ptr);
    }
}
