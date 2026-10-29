pragma solcore;

trait Marker<a> {}
contract C<t> {
    #[derive(Marker)]
    enum Local {
        Local(t)
    }
}
