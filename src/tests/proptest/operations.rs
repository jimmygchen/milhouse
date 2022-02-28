use super::{arb_hash256, arb_index, arb_list, arb_vect};
use crate::{Diff, Error, List, ListDiff, Vector, VectorDiff};
use proptest::prelude::*;
use ssz::{Decode, Encode};
use ssz_derive::{Decode, Encode};
use std::fmt::Debug;
use std::marker::PhantomData;
use tree_hash::{Hash256, TreeHash};
use tree_hash_derive::TreeHash;
use typenum::{Unsigned, U1, U1024, U2, U3, U32, U33, U4, U7, U8, U9};

const OP_LIMIT: usize = 32;

/// Simple specification for `List` and `Vector` behaviour.
#[derive(Debug, Clone)]
pub struct Spec<T, N: Unsigned> {
    values: Vec<T>,
    allow_push: bool,
    _phantom: PhantomData<N>,
}

impl<T, N: Unsigned> Spec<T, N> {
    pub fn list(values: Vec<T>) -> Self {
        assert!(values.len() <= N::to_usize());
        Self {
            values,
            allow_push: true,
            _phantom: PhantomData,
        }
    }

    pub fn vect(values: Vec<T>) -> Self {
        assert_eq!(values.len(), N::to_usize());
        Self {
            values,
            allow_push: false,
            _phantom: PhantomData,
        }
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.values.iter()
    }

    pub fn iter_from(&self, index: usize) -> Result<impl Iterator<Item = &T>, Error> {
        if index <= self.len() {
            Ok(self.values[index..].iter())
        } else {
            Err(Error::OutOfBoundsIterFrom {
                index,
                len: self.len(),
            })
        }
    }

    pub fn get(&self, index: usize) -> Option<&T> {
        self.values.get(index)
    }

    pub fn set(&mut self, index: usize, value: T) -> Option<()> {
        *self.values.get_mut(index)? = value;
        Some(())
    }

    pub fn push(&mut self, value: T) -> Result<(), Error> {
        if self.allow_push {
            if self.values.len() == N::to_usize() {
                return Err(Error::ListFull {
                    len: self.values.len(),
                });
            }
            self.values.push(value);
            Ok(())
        } else {
            Err(Error::PushNotSupported)
        }
    }
}

#[derive(Debug, Clone)]
pub enum Op<T> {
    /// Check that `len` returns the correct length.
    Len,
    /// Check that `get` returns the correct value for a given index.
    Get(usize),
    /// Use `get_mut` to set an element at a given index.
    Set(usize, T),
    /// Use `get_cow` to set an element at a given index.
    SetCow(usize, T),
    /// Use `push` to try to add a new element to the list.
    Push(T),
    /// Check the `iter` method.
    Iter,
    /// Check the `iter_from` method.
    IterFrom(usize),
    /// Apply updates to the backing list.
    ApplyUpdates,
    /// Set the current state of the list as the checkpoint for the next diff.
    DiffCheckpoint,
    /// Compute a diff with respect to the most recent checkpoint and verify its correctness.
    DiffCompute,
}

fn arb_op<'a, T, S>(strategy: &'a S, n: usize) -> impl Strategy<Value = Op<T>> + 'a
where
    T: Debug + Clone + 'a,
    S: Strategy<Value = T> + 'a,
{
    prop_oneof![
        Just(Op::Len),
        arb_index(n).prop_map(Op::Get),
        (arb_index(n), strategy).prop_map(|(index, value)| Op::Set(index, value)),
        (arb_index(n), strategy).prop_map(|(index, value)| Op::SetCow(index, value)),
        strategy.prop_map(Op::Push),
        Just(Op::Iter),
        arb_index(n).prop_map(Op::IterFrom),
        Just(Op::ApplyUpdates),
        Just(Op::DiffCheckpoint),
        Just(Op::DiffCompute),
    ]
}

fn arb_ops<'a, T, S>(
    strategy: &'a S,
    n: usize,
    limit: usize,
) -> impl Strategy<Value = Vec<Op<T>>> + 'a
where
    T: Debug + Clone + 'a,
    S: Strategy<Value = T> + 'a,
{
    proptest::collection::vec(arb_op(strategy, n), 1..limit)
}

fn apply_ops_list<T, N>(list: &mut List<T, N>, spec: &mut Spec<T, N>, ops: Vec<Op<T>>)
where
    T: TreeHash + PartialEq + Clone + Encode + Decode + Debug,
    N: Unsigned + Debug,
{
    let mut diff_checkpoint = list.clone();

    for op in ops {
        match op {
            Op::Len => {
                assert_eq!(list.len(), spec.len())
            }
            Op::Get(index) => {
                assert_eq!(list.get(index), spec.get(index));
            }
            Op::Set(index, value) => {
                let res = list.get_mut(index).map(|elem| *elem = value.clone());
                assert_eq!(res, spec.set(index, value));
            }
            Op::SetCow(index, value) => {
                let res = list.get_cow(index).map(|cow| *cow.to_mut() = value.clone());
                assert_eq!(res, spec.set(index, value));
            }
            Op::Push(value) => {
                assert_eq!(list.push(value.clone()), spec.push(value));
            }
            Op::Iter => {
                assert!(list.iter().eq(spec.iter()));
            }
            Op::IterFrom(index) => match (list.iter_from(index), spec.iter_from(index)) {
                (Ok(iter1), Ok(iter2)) => assert!(iter1.eq(iter2)),
                (Err(e1), Err(e2)) => assert_eq!(e1, e2),
                (Err(e), _) | (_, Err(e)) => panic!("iter_from mismatch: {}", e),
            },
            Op::ApplyUpdates => {
                list.apply_updates().unwrap();
            }
            Op::DiffCheckpoint => {
                list.apply_updates().unwrap();
                diff_checkpoint = list.clone();
            }
            Op::DiffCompute => {
                list.apply_updates().unwrap();
                let diff = ListDiff::compute_diff(&diff_checkpoint, list).unwrap();
                let mut diffed_list = diff_checkpoint.clone();
                diff.apply_diff(&mut diffed_list).unwrap();
                assert_eq!(diffed_list, *list);
            }
        }
    }
}

fn apply_ops_vect<T, N>(vect: &mut Vector<T, N>, spec: &mut Spec<T, N>, ops: Vec<Op<T>>)
where
    T: TreeHash + PartialEq + Clone + Encode + Decode + Debug,
    N: Unsigned + Debug,
{
    let mut diff_checkpoint = vect.clone();

    for op in ops {
        match op {
            Op::Len => {
                assert_eq!(vect.len(), spec.len())
            }
            Op::Get(index) => {
                assert_eq!(vect.get(index), spec.get(index));
            }
            Op::Set(index, value) => {
                let res = vect.get_mut(index).map(|elem| *elem = value.clone());
                assert_eq!(res, spec.set(index, value));
            }
            Op::SetCow(index, value) => {
                let res = vect.get_cow(index).map(|cow| *cow.to_mut() = value.clone());
                assert_eq!(res, spec.set(index, value));
            }
            Op::Push(_) => {
                // No-op
            }
            Op::Iter => {
                assert!(vect.iter().eq(spec.iter()));
            }
            Op::IterFrom(index) => match (vect.iter_from(index), spec.iter_from(index)) {
                (Ok(iter1), Ok(iter2)) => assert!(iter1.eq(iter2)),
                (Err(e1), Err(e2)) => assert_eq!(e1, e2),
                (Err(e), _) | (_, Err(e)) => panic!("iter_from mismatch: {}", e),
            },
            Op::ApplyUpdates => {
                vect.apply_updates().unwrap();
            }
            Op::DiffCheckpoint => {
                vect.apply_updates().unwrap();
                diff_checkpoint = vect.clone();
            }
            Op::DiffCompute => {
                vect.apply_updates().unwrap();
                let diff = VectorDiff::compute_diff(&diff_checkpoint, vect).unwrap();
                let mut diffed_vect = diff_checkpoint.clone();
                diff.apply_diff(&mut diffed_vect).unwrap();
                assert_eq!(diffed_vect, *vect);
            }
        }
    }
}

macro_rules! list_test {
    ($name:ident, $T:ty, $N:ty) => {
        // Use default strategy (assumes existence of an `Arbitrary` impl).
        list_test!($name, $T, $N, any::<$T>());
    };
    ($name:ident, $T:ty, $N:ty, $strat:expr) => {
        proptest! {
            #[test]
            fn $name(
                init in arb_list::<$T, $N, _>(&$strat),
                ops in arb_ops::<$T, _>(&$strat, <$N>::to_usize(), OP_LIMIT)
            ) {
                let mut list = List::<$T, $N>::new(init.clone()).unwrap();
                let mut spec = Spec::<$T, $N>::list(init);
                apply_ops_list(&mut list, &mut spec, ops);
            }
        }
    };
}

macro_rules! vect_test {
    ($name:ident, $T:ty, $N:ty) => {
        // Use default strategy (assumes existence of an `Arbitrary` impl).
        vect_test!($name, $T, $N, any::<$T>());
    };
    ($name:ident, $T:ty, $N:ty, $strat:expr) => {
        proptest! {
            #[test]
            fn $name(
                init in arb_vect::<$T, $N, _>(&$strat),
                ops in arb_ops::<$T, _>(&$strat, <$N>::to_usize(), OP_LIMIT)
            ) {
                let mut vect = Vector::<$T, $N>::new(init.clone()).unwrap();
                let mut spec = Spec::<$T, $N>::vect(init);
                apply_ops_vect(&mut vect, &mut spec, ops);
            }
        }
    };
}

#[derive(Debug, Clone, PartialEq, Encode, Decode, TreeHash)]
struct Large {
    a: u64,
    b: u8,
    c: Hash256,
    d: List<u64, U4>,
}

fn arb_large() -> impl Strategy<Value = Large> {
    (
        any::<u64>(),
        any::<u8>(),
        arb_hash256(),
        arb_list::<_, U4, _>(any::<u64>()),
    )
        .prop_map(|(a, b, c, d)| Large {
            a,
            b,
            c,
            d: List::new(d).unwrap(),
        })
}

mod list {
    use super::*;

    list_test!(u8_1, u8, U1);
    list_test!(u8_2, u8, U2);
    list_test!(u8_3, u8, U3);
    list_test!(u8_4, u8, U4);
    list_test!(u8_7, u8, U7);
    list_test!(u8_8, u8, U8);
    list_test!(u8_9, u8, U9);
    list_test!(u8_32, u8, U32);
    list_test!(u8_33, u8, U33);
    list_test!(u8_1024, u8, U1024);

    list_test!(u64_1, u64, U1);
    list_test!(u64_2, u64, U2);
    list_test!(u64_3, u64, U3);
    list_test!(u64_4, u64, U4);
    list_test!(u64_7, u64, U7);
    list_test!(u64_8, u64, U8);
    list_test!(u64_9, u64, U9);
    list_test!(u64_32, u64, U32);
    list_test!(u64_33, u64, U33);
    list_test!(u64_1024, u64, U1024);

    list_test!(hash256_1, Hash256, U1, arb_hash256());
    list_test!(hash256_2, Hash256, U2, arb_hash256());
    list_test!(hash256_3, Hash256, U3, arb_hash256());
    list_test!(hash256_4, Hash256, U4, arb_hash256());
    list_test!(hash256_7, Hash256, U7, arb_hash256());
    list_test!(hash256_8, Hash256, U8, arb_hash256());
    list_test!(hash256_9, Hash256, U9, arb_hash256());
    list_test!(hash256_32, Hash256, U32, arb_hash256());
    list_test!(hash256_33, Hash256, U33, arb_hash256());
    list_test!(hash256_1024, Hash256, U1024, arb_hash256());

    list_test!(large_1, Large, U1, arb_large());
    list_test!(large_2, Large, U2, arb_large());
    list_test!(large_3, Large, U3, arb_large());
    list_test!(large_4, Large, U4, arb_large());
    list_test!(large_7, Large, U7, arb_large());
    list_test!(large_8, Large, U8, arb_large());
    list_test!(large_9, Large, U9, arb_large());
    list_test!(large_32, Large, U32, arb_large());
    list_test!(large_33, Large, U33, arb_large());
    list_test!(large_1024, Large, U1024, arb_large());
}

mod vect {
    use super::*;

    vect_test!(u8_1, u8, U1);
    vect_test!(u8_2, u8, U2);
    vect_test!(u8_3, u8, U3);
    vect_test!(u8_4, u8, U4);
    vect_test!(u8_7, u8, U7);
    vect_test!(u8_8, u8, U8);
    vect_test!(u8_9, u8, U9);
    vect_test!(u8_32, u8, U32);
    vect_test!(u8_33, u8, U33);
    vect_test!(u8_1024, u8, U1024);

    vect_test!(u64_1, u64, U1);
    vect_test!(u64_2, u64, U2);
    vect_test!(u64_3, u64, U3);
    vect_test!(u64_4, u64, U4);
    vect_test!(u64_7, u64, U7);
    vect_test!(u64_8, u64, U8);
    vect_test!(u64_9, u64, U9);
    vect_test!(u64_32, u64, U32);
    vect_test!(u64_33, u64, U33);
    vect_test!(u64_1024, u64, U1024);

    vect_test!(hash256_1, Hash256, U1, arb_hash256());
    vect_test!(hash256_2, Hash256, U2, arb_hash256());
    vect_test!(hash256_3, Hash256, U3, arb_hash256());
    vect_test!(hash256_4, Hash256, U4, arb_hash256());
    vect_test!(hash256_7, Hash256, U7, arb_hash256());
    vect_test!(hash256_8, Hash256, U8, arb_hash256());
    vect_test!(hash256_9, Hash256, U9, arb_hash256());
    vect_test!(hash256_32, Hash256, U32, arb_hash256());
    vect_test!(hash256_33, Hash256, U33, arb_hash256());
    vect_test!(hash256_1024, Hash256, U1024, arb_hash256());

    vect_test!(large_1, Large, U1, arb_large());
    vect_test!(large_2, Large, U2, arb_large());
    vect_test!(large_3, Large, U3, arb_large());
    vect_test!(large_4, Large, U4, arb_large());
    vect_test!(large_7, Large, U7, arb_large());
    vect_test!(large_8, Large, U8, arb_large());
    vect_test!(large_9, Large, U9, arb_large());
    vect_test!(large_32, Large, U32, arb_large());
    vect_test!(large_33, Large, U33, arb_large());
    vect_test!(large_1024, Large, U1024, arb_large());
}