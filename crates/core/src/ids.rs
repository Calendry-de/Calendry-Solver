//! Dense index newtypes.
//!
//! Everything in the hot loop addresses entities by `u32` index into a flat
//! array, never by the `String` UUID that arrives on the wire. String ids are
//! resolved exactly once at load and restored exactly once at output.

macro_rules! idx {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
        pub struct $name(pub u32);

        impl $name {
            #[inline]
            pub fn get(self) -> usize {
                self.0 as usize
            }
        }
    };
}

idx!(
    /// A single block of the tenant's grid, flattened across (week, day, block).
    SlotIdx
);
idx!(RoomIdx);
idx!(GroupIdx);
idx!(PersonIdx);
idx!(OfferingIdx);
idx!(
    /// One Session that needs placing: an (Offering, occurrence) pair.
    PlacementIdx
);
