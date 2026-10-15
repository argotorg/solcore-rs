import * from std;
import * from std.dispatch;

// #[scenario("payable")]
// non-payable constructor accepts zero value
// #[deploy(0x) -> success]
// #[raw(0xd0e30db0, value = 42, label = "deposit()(uint256)") -> success(0x000000000000000000000000000000000000000000000000000000000000002a)]
// #[raw(0xd0e30db0, label = "deposit()(uint256)") -> success(0x0000000000000000000000000000000000000000000000000000000000000000)]
// #[raw(0xb69ef8a8, label = "balance()(uint256)") -> success(0x000000000000000000000000000000000000000000000000000000000000002a)]
// #[raw(0xb69ef8a8, value = 42, label = "balance()(uint256)") -> revert(0xb5988ea3)]
// #[raw(0xdeadc0de, value = 42, label = "fallback with value") -> success(0x)]
// #[raw(0xdeadc0de, label = "fallback without value should fail") -> revert(0x66616c6c6261636b2d7761732d63616c6c65642d6e6f2d76616c7565)]
// #[raw(0xb69ef8a8, label = "balance()(uint256)") -> success(0x0000000000000000000000000000000000000000000000000000000000000054)]
contract PayableTest {
    constructor() {}

    function deposit() public payable returns (uint256) {
        let value;
        assembly {
            value := callvalue()
        }
        return uint256(value);
    }

    function balance() public returns (uint256) {
        let value;
        assembly {
            value := selfbalance()
        }
        return uint256(value);
    }

    fallback() payable {
        let value;
        assembly {
            value := callvalue()
        }
        if (value == 0) {
            revertLit("fallback-was-called-no-value");
        }
    }
}
