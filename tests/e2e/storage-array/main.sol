import * from std;
import * from std.dispatch;
import { mload, mstore } from std.opcodes;

// #[scenario("storage")]
// constructor()
// #[deploy(0x) -> success]
// #[raw(0xa30e3fa9, label = "numberOfMembers()(uint256) - initially 0") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0x9eab5253, label = "getMembers()(address[]) - initially empty") -> success(0x00000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0xca6d56dc0000000000000000000000000000000000000000000000000000000000000001, label = "addMember(0x1)") -> success(0x)]
// #[raw(0xca6d56dc0000000000000000000000000000000000000000000000000000000000000002, label = "addMember(0x2)") -> success(0x)]
// #[raw(0xca6d56dc0000000000000000000000000000000000000000000000000000000000000003, label = "addMember(0x3)") -> success(0x)]
// #[raw(0xa30e3fa9, label = "numberOfMembers()(uint256) - now 3") -> success(0x0000000000000000000000000000000000000000000000000000000000000003)]
// #[raw(0x9eab5253, label = "getMembers()(address[]) - [0x1, 0x2, 0x3]") -> success(0x00000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000003000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000000003)]
// #[raw(0x0b1ca49a0000000000000000000000000000000000000000000000000000000000000002, label = "removeMember(0x2) - swap-pop, 0x3 moves to index 1") -> success(0x)]
// #[raw(0xa30e3fa9, label = "numberOfMembers()(uint256) - now 2") -> success(0x0000000000000000000000000000000000000000000000000000000000000002)]
// #[raw(0x9eab5253, label = "getMembers()(address[]) - [0x1, 0x3]") -> success(0x0000000000000000000000000000000000000000000000000000000000000020000000000000000000000000000000000000000000000000000000000000000200000000000000000000000000000000000000000000000000000000000000010000000000000000000000000000000000000000000000000000000000000003)]
// #[raw(0x0b1ca49a0000000000000000000000000000000000000000000000000000000000000009, label = "removeMember(0x9) - not in array, should revert with MemberNotFound()") -> revert(0xdeadbeef)]
contract MemberRegistry {
    members: array<address>;

    constructor() {}

    function addMember(addr: address) public {
        ArrayPush.push(members, addr);
    }

    // MemberNotFound() selector
    function removeMember(addr: address) public {
        // foundIdx == length() acts as the "not found" sentinel.
        let foundIdx: uint256 = Length.length(members);
        let i: uint256;
        for (i = uint256(0); i < Length.length(members); i = i + uint256(1)) {
            if (members[i] == addr) {
                // NOTE: solcore has no `break`, so we keep scanning.
                foundIdx = i;
            }
        }
        require(foundIdx != Length.length(members), Error(0xdeadbeef));

        // Shift subsequent elements down one slot to close the gap.
        for (; foundIdx < Length.length(members) - uint256(1); foundIdx = foundIdx + uint256(1)) {
            members[foundIdx] = members[foundIdx + uint256(1)];
        }
        // Drop the (now-duplicated) last item and adjust the length.
        Array.pop(members);
    }

    function numberOfMembers() public returns (uint256) {
        return Length.length(members);
    }

    function getMembers() public returns (memory<DynArray<address>>) {
        let count: word = Typedef.rep(Length.length(members));
        let totalBytes: word = (count + 1) * 32;
        let ptr: word = allocate_memory(totalBytes);
        mstore(ptr, count);

        let i: word;
        for (i = 0; i < count; i = i + 1) {
            let addr: address = members[uint256(i)];
            mstore(ptr + 32 + i * 32, Typedef.rep(addr));
        }
        return Typedef.abs(ptr);
    }
}
