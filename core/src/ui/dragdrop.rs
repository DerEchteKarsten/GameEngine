//! Drag-and-drop payload store: one type-erased payload shared by all UI windows, and lazily loaded asset payloads.
use std::{
    any::{Any, TypeId},
    sync::Mutex,
};

use bevy::{
    asset::{Asset, Handle, UntypedHandle},
    ecs::{resource::Resource, system::Res},
    input::{ButtonInput, mouse::MouseButton, touch::Touches},
};

type Payload = Box<dyn Any + Send + Sync>;

#[derive(Resource, Default)]
pub struct DragDrop {
    payload: Mutex<Option<Payload>>,
}

impl DragDrop {
    pub fn begin<T: Any + Send + Sync>(&self, value: T) {
        if let Ok(mut payload) = self.payload.lock() {
            *payload = Some(Box::new(value));
        }
    }

    pub fn active(&self) -> bool {
        self.payload.lock().is_ok_and(|p| p.is_some())
    }

    /// Whether the payload is a `T` that satisfies `pred`.
    pub fn accepts<T: Any>(&self, pred: impl FnOnce(&T) -> bool) -> bool {
        self.payload
            .lock()
            .ok()
            .and_then(|p| p.as_deref().and_then(|p| p.downcast_ref::<T>()).map(pred))
            .unwrap_or(false)
    }

    /// Removes and returns the payload if it is a `T` that satisfies `pred`. Any other payload
    /// is left in place for the next target.
    pub fn take<T: Any>(&self, pred: impl FnOnce(&T) -> bool) -> Option<T> {
        let mut payload = self.payload.lock().ok()?;
        if !payload.as_deref()?.downcast_ref::<T>().is_some_and(pred) {
            return None;
        }
        let value: Box<dyn Any> = payload.take()?;
        value.downcast::<T>().ok().map(|v| *v)
    }

    pub fn clear(&self) {
        if let Ok(mut payload) = self.payload.lock() {
            *payload = None;
        }
    }

    /// Whether the payload is an `AssetDrag` of an `A`.
    pub fn accepts_asset<A: Asset>(&self) -> bool {
        self.accepts(AssetDrag::is::<A>)
    }

    /// Takes the payload if it is an `AssetDrag` of an `A`, and loads the asset.
    pub fn take_asset<A: Asset>(&self) -> Option<Handle<A>> {
        self.take(AssetDrag::is::<A>).and_then(AssetDrag::load)
    }
}

/// A dragged asset that is not loaded yet: `load` only runs once a target takes the drop, so
/// dragging something around costs nothing.
pub struct AssetDrag {
    type_id: TypeId,
    load: Box<dyn FnOnce() -> UntypedHandle + Send + Sync>,
}

impl AssetDrag {
    /// `load` has to return a handle to an asset of type `type_id`.
    pub fn new(
        type_id: TypeId,
        load: impl FnOnce() -> UntypedHandle + Send + Sync + 'static,
    ) -> Self {
        Self {
            type_id,
            load: Box::new(load),
        }
    }

    pub fn is<A: Asset>(&self) -> bool {
        self.type_id == TypeId::of::<A>()
    }

    pub fn load<A: Asset>(self) -> Option<Handle<A>> {
        (self.load)().try_typed::<A>().ok()
    }
}

/// Clears the payload once the primary button/touch is no longer held. Runs in `PostUpdate`,
/// so every `Update` drop target saw the payload on the release frame.
pub fn end_drag(dnd: Res<DragDrop>, mouse: Res<ButtonInput<MouseButton>>, touch: Res<Touches>) {
    if !mouse.pressed(MouseButton::Left) && touch.iter().next().is_none() {
        dnd.clear();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use bevy::{asset::Asset, reflect::TypePath};

    use super::*;

    #[derive(Asset, TypePath)]
    struct Apple;

    #[derive(Asset, TypePath)]
    struct Pear;

    #[test]
    fn empty_store_accepts_nothing() {
        let dnd = DragDrop::default();
        assert!(!dnd.active());
        assert!(!dnd.accepts::<u32>(|_| true));
        assert_eq!(dnd.take::<u32>(|_| true), None);
    }

    #[test]
    fn take_empties_the_store() {
        let dnd = DragDrop::default();
        dnd.begin(7u32);
        assert!(dnd.active());
        assert!(dnd.accepts::<u32>(|v| *v == 7));
        assert_eq!(dnd.take::<u32>(|_| true), Some(7));
        assert!(!dnd.active());
        assert_eq!(dnd.take::<u32>(|_| true), None);
    }

    #[test]
    fn type_mismatch_leaves_the_payload() {
        let dnd = DragDrop::default();
        dnd.begin(7u32);
        assert!(!dnd.accepts::<String>(|_| true));
        assert_eq!(dnd.take::<String>(|_| true), None);
        assert_eq!(dnd.take::<u32>(|_| true), Some(7));
    }

    #[test]
    fn rejected_predicate_leaves_the_payload() {
        let dnd = DragDrop::default();
        dnd.begin(7u32);
        assert!(!dnd.accepts::<u32>(|v| *v == 8));
        assert_eq!(dnd.take::<u32>(|v| *v == 8), None);
        assert!(dnd.active());
    }

    #[test]
    fn begin_replaces_and_clear_drops() {
        let dnd = DragDrop::default();
        dnd.begin(7u32);
        dnd.begin("pear".to_string());
        assert!(!dnd.accepts::<u32>(|_| true));
        assert!(dnd.accepts::<String>(|s| s == "pear"));
        dnd.clear();
        assert!(!dnd.active());
    }

    #[test]
    fn dragged_assets_match_on_type_and_load_when_taken() {
        static LOADS: AtomicUsize = AtomicUsize::new(0);
        let dnd = DragDrop::default();
        dnd.begin(AssetDrag::new(TypeId::of::<Apple>(), || {
            LOADS.fetch_add(1, Ordering::Relaxed);
            Handle::<Apple>::default().untyped()
        }));
        assert!(dnd.accepts_asset::<Apple>());
        assert!(!dnd.accepts_asset::<Pear>());
        assert!(dnd.take_asset::<Pear>().is_none());
        assert!(dnd.active());
        assert_eq!(
            LOADS.load(Ordering::Relaxed),
            0,
            "nothing took the drop yet"
        );
        assert_eq!(dnd.take_asset::<Apple>(), Some(Handle::<Apple>::default()));
        assert_eq!(LOADS.load(Ordering::Relaxed), 1);
        assert!(!dnd.active());
    }
}
