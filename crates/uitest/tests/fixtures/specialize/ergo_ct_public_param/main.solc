// comptime parameter on a *public* contract entry point.  Public entry
// arguments come from calldata at runtime, so this can never be satisfied.
// Should be rejected with a clear "public functions cannot take comptime
// parameters" style error.
import std.{*};
import std.dispatch.{*};

contract CtPublicParam {
  public function double(comptime x : word) -> word {
    return x + x;
  }
}
