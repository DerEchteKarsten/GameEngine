use std::{any::TypeId, sync::RwLock};

use bevy::{
    asset::{Asset, Handle, UntypedHandle},
    ecs::{entity::Entity, resource::Resource},
};

use crate::ui::builder::DraggableState;

struct Item {
    handle: UntypedHandle,
    tid: TypeId,
}

#[derive(Resource, Default)]
pub struct AssetDragAndDropProvider {
    item: RwLock<Option<Item>>,
}

impl AssetDragAndDropProvider {
    pub fn drag<A: Asset>(&self, state: DraggableState, handle: Handle<A>) {
        if state.drag_started {
            let Ok(mut item) = self.item.write() else {
                return;
            };

            *item = Some(Item {
                handle: handle.untyped(),
                tid: TypeId::of::<A>(),
            });
        }

        if state.dropped {
            let Ok(mut item) = self.item.write() else {
                return;
            };

            *item = None;
        }
    }

    pub fn drop<A: Asset>(&self) -> Option<Handle<A>> {
        if let Ok(mut item) = self.item.write()
            && let Some(item) = item.take()
        {
            if item.tid == TypeId::of::<A>() {
                return item.handle.try_typed::<A>().ok();
            }
        }
        None
    }

    pub fn drop_valid<A: Asset>(&self) -> bool {
        if let Ok(item) = self.item.read()
            && let Some(item) = item.as_ref()
        {
            item.tid == TypeId::of::<A>()
        } else {
            false
        }
    }
}

#[derive(Resource, Default)]
pub struct EntityDragAndDropProvider {
    entity: RwLock<Option<Entity>>,
}

impl EntityDragAndDropProvider {
    pub fn drag(&self, state: DraggableState, entity: Entity) {
        if state.drag_started {
            let Ok(mut item) = self.entity.write() else {
                return;
            };

            *item = Some(entity);
        }

        if state.dropped {
            let Ok(mut item) = self.entity.write() else {
                return;
            };

            *item = None;
        }
    }

    pub fn drop(&self) -> Option<Entity> {
        if let Ok(mut item) = self.entity.write()
            && let Some(item) = item.take()
        {
            return Some(item);
        }
        None
    }

    pub fn drop_valid(&self) -> bool {
        self.entity.read().ok().and_then(|s| *s).is_some()
    }
}
