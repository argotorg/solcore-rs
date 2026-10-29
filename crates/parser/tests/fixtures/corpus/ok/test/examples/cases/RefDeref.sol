pragma solcore;

trait Loadable<ref, deref> {
    function load(r: ref) returns (deref);
}

trait Storable<ref, deref> {
    function store(r: ref, d: deref);
}

// trait constraints
trait Ref<ref, deref> where ref: Loadable<deref>, ref: Storable<ref> {}
