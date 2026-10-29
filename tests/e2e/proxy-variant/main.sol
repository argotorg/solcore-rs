pragma solcore;

enum Proxy<t> {
    Other,
    Proxy
}

function choose(proxy: Proxy<word>) returns (word) {
    match (proxy) {
        case Proxy.Other {
            return 0;
        }
        case Proxy.Proxy {
            return 42;
        }
    }
}

// #[scenario("proxy_variant")]
// #[deploy(0x) -> success]
// #[raw(0x, label = "suite `proxy_variant` test #2") -> success(0x000000000000000000000000000000000000000000000000000000000000002a)]
contract ProxyVariant {
    function main() returns (word) {
        let chosen: comptime<word> = choose(@word);
        assembly {
            mstore(0, chosen)
            return (0, 32)
        }
        return 0;
    }
}
