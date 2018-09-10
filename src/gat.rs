// From https://lukaskalbertodt.github.io/2018/08/03/solving-the-generalized-streaming-iterator-problem-without-gats.html

// The family trait for type constructors that have one
// input lifetime.
trait FamilyLt<'a> {
    type Out;
}

// A family which represents a type constructor that always
// returns `T` (thus "id").
struct IdFamily<T: ?Sized>(PhantomData<T>, !);
impl<'a, T: ?Sized> FamilyLt<'a> for IdFamily<T> {
    type Out = T;
}

// Represents references to `T`.
struct RefFamily<T: ?Sized>(PhantomData<T>, !);
impl<'a, T: 'a + ?Sized> FamilyLt<'a> for RefFamily<T> {
    type Out = &'a T;
}
