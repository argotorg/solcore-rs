import * from std;
import {Option} from option;

export { TransferHook, NoHook(*), Stacked(*) };

// A transfer hook runs around every balance change, before or after the
// effects. from = None is a mint, to = None is a burn.
trait TransferHook<h> {
    function on(hook: h, from: Option<address>, to: Option<address>, amount: uint256);
}

// The empty hook chain. Monomorphization erases it, so a side with no
// hooks costs nothing.
enum NoHook { NoHook }

impl TransferHook<NoHook> {
    function on(hook: NoHook, from: Option<address>, to: Option<address>, amount: uint256) {
    }
}

// Hooks compose as values, applied left to right. The order is the
// expression written at the use site.
enum Stacked<f, g> { Stacked(f, g) }

impl<f, g> TransferHook<Stacked<f, g>> where f: TransferHook, g: TransferHook {
    function on(hook: Stacked<f, g>, from: Option<address>, to: Option<address>, amount: uint256) {
        match (hook) {
            case Stacked(first, second) {
                TransferHook.on(first, from, to, amount);
                TransferHook.on(second, from, to, amount);
            }
        }
    }
}
