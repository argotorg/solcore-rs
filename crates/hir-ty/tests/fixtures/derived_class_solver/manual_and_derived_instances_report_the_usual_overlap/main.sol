pragma solcore;

trait Marker<a> {}
#[derive(Marker)]
enum Target {}
impl Marker<Target> {}
