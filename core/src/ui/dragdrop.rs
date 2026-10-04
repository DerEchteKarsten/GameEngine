//! Drag-and-drop payload store: one type-erased payload shared by all UI windows.
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

    /// Whether the payload is an `UntypedHandle` to an `A`.
    pub fn accepts_asset<A: Asset>(&self) -> bool {
        self.accepts(is_handle_of::<A>)
    }

    /// Takes the payload if it is an `UntypedHandle` to an `A`.
    pub fn take_asset<A: Asset>(&self) -> Option<Handle<A>> {
        self.take(is_handle_of::<A>)
            .and_then(|handle| handle.try_typed::<A>().ok())
    }
}

pub(crate) fn is_handle_of<A: Asset>(handle: &UntypedHandle) -> bool {
    UntypedHandle::type_id(handle) == TypeId::of::<A>()
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
    fn asset_handles_match_on_asset_type() {
        let dnd = DragDrop::default();
        dnd.begin(Handle::<Apple>::default().untyped());
        assert!(dnd.accepts_asset::<Apple>());
        assert!(!dnd.accepts_asset::<Pear>());
        assert!(dnd.take_asset::<Pear>().is_none());
        assert!(dnd.active());
        assert_eq!(dnd.take_asset::<Apple>(), Some(Handle::<Apple>::default()));
        assert!(!dnd.active());
    }
}
