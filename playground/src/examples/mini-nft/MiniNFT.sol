import * from std;
import * from std.dispatch;
import { Option, contains } from option;

// A token either has an owner or does not exist: unset mapping entries
// read back as Option.None.
contract MiniNFT {
    nextId: uint256;
    owners: mapping(uint256 => Option<address>);
    approvals: mapping(uint256 => Option<address>);
    balances: mapping(address => uint256);

    constructor() {}

    function mint() public returns (uint256) {
        let id = nextId;
        nextId = nextId + uint256(1);
        let to = msgSender();
        owners[id] = Option.Some(to);
        balances[to] = balances[to] + uint256(1);
        return id;
    }

    function ownerOf(id: uint256) public returns (address) {
        match (owners[id]) {
            case Option.Some(owner) {
                return owner;
            }
            default {
                require(false, "no such token");
                return address(0);
            }
        }
    }

    function approve(to: address, id: uint256) public {
        require(msgSender() == ownerOf(id), "not the owner");
        approvals[id] = Option.Some(to);
    }

    function transfer(to: address, id: uint256) public {
        let owner = ownerOf(id);
        let from = msgSender();
        require(from == owner || contains(approvals[id], from), "not authorized");
        approvals[id] = Option.None;
        owners[id] = Option.Some(to);
        balances[owner] = balances[owner] - uint256(1);
        balances[to] = balances[to] + uint256(1);
    }

    function balanceOf(who: address) public returns (uint256) {
        return balances[who];
    }

    function exists(id: uint256) public returns (bool) {
        match (owners[id]) {
            case Option.Some(_) {
                return true;
            }
            default {
                return false;
            }
        }
    }
}
