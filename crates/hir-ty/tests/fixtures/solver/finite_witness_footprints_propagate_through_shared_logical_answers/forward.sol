pragma solcore;

trait Target<a> {}
trait Super<a> where a: Target {}
trait Bridge<a> {}
trait Leaf<a> {}
trait RecursiveLeaf<a> where a: Leaf {}
trait IndependentLeaf<a> where a: Leaf {}
trait D<a> {}
enum Box<a> {
    Box(a)
}
trait Slow0<a> {}
trait Slow1<a> {}
trait Slow2<a> {}
trait Slow3<a> {}
trait Slow4<a> {}
trait Slow5<a> {}
trait Slow6<a> {}
trait Slow7<a> {}
impl<a> Target<a> where a: Bridge {}
impl<a> Bridge<a> where a: Leaf {}
impl<a> RecursiveLeaf<a> where Box<a>: D {}
impl<a> IndependentLeaf<a> where a: Slow0 {}
impl<a> Super<a> {}
impl D<word> {}
impl<a> D<Box<a>> where a: D, a: Target {}
impl<a> Slow0<a> where a: Slow1 {}
impl<a> Slow1<a> where a: Slow2 {}
impl<a> Slow2<a> where a: Slow3 {}
impl<a> Slow3<a> where a: Slow4 {}
impl<a> Slow4<a> where a: Slow5 {}
impl<a> Slow5<a> where a: Slow6 {}
impl<a> Slow6<a> where a: Slow7 {}
impl<a> Slow7<a> {}
