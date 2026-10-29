import * from std;
import { caller } from std.opcodes;

export { sender };

// The address making the current call.
function sender() returns (address) {
    return address(caller());
}
