use ::bevy::prelude::*;
#[cfg(feature = "bevy_bindings")]
pub mod bevy_bindings;
pub mod core;

#[cfg(feature = "test_functions")]
pub mod test_functions;

pub use core::*;

pub struct ScriptFunctionsPlugin;

impl Plugin for ScriptFunctionsPlugin {
    fn build(&self, app: &mut App) {
        register_bevy_bindings(app);
        register_core_functions(app);
        #[cfg(feature = "test_functions")]
        test_functions::register_test_functions(app);

        // TODO: if bevy ever does this itself we should remove this
        app.world_mut().register_component::<Parent>();
        app.world_mut().register_component::<Children>();
    }
}
