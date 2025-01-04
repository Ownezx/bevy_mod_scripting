#![allow(clippy::arc_with_non_send_sync)]

use crate::event::ScriptErrorEvent;
use asset::{AssetIdToScriptIdMap, ScriptAsset, ScriptAssetLoader, ScriptAssetSettings};
use bevy::prelude::*;
use bindings::{
    function::script_function::AppScriptFunctionRegistry, script_value::ScriptValue,
    AppReflectAllocator, ReflectAllocator, ReflectReference, ScriptTypeRegistration,
    WorldCallbackAccess,
};
use context::{
    Context, ContextAssigner, ContextBuilder, ContextInitializer, ContextLoadingSettings,
    ContextPreHandlingInitializer, ScriptContexts,
};
use docs::{Documentation, DocumentationFragment};
use event::ScriptCallbackEvent;
use handler::{CallbackSettings, HandlerFn};

use runtime::{Runtime, RuntimeContainer, RuntimeInitializer, RuntimeSettings};
use script::Scripts;
use systems::{garbage_collector, initialize_runtime, sync_script_data};

pub mod asset;
pub mod bindings;
pub mod commands;
pub mod context;
pub mod docs;
pub mod error;
pub mod event;
pub mod handler;
pub mod reflection_extensions;
pub mod runtime;
pub mod script;
pub mod systems;
pub mod world;

/// Types which act like scripting plugins, by selecting a context and runtime
/// Each individual combination of context and runtime has specific infrastructure built for it and does not interact with other scripting plugins
pub trait IntoScriptPluginParams: 'static {
    type C: Context;
    type R: Runtime;
}

/// Bevy plugin enabling scripting within the bevy mod scripting framework
pub struct ScriptingPlugin<P: IntoScriptPluginParams> {
    /// Callback for initiating the runtime
    pub runtime_builder: fn() -> P::R,
    /// Settings for the runtime
    pub runtime_settings: Option<RuntimeSettings<P>>,
    /// The handler used for executing callbacks in scripts
    pub callback_handler: Option<HandlerFn<P>>,
    /// The context builder for loading contexts
    pub context_builder: Option<ContextBuilder<P>>,
    /// The context assigner for assigning contexts to scripts, if not provided default strategy of keeping each script in its own context is used
    pub context_assigner: Option<ContextAssigner<P>>,
}

impl<P: IntoScriptPluginParams> Default for ScriptingPlugin<P>
where
    P::R: Default,
{
    fn default() -> Self {
        Self {
            runtime_builder: P::R::default,
            runtime_settings: Default::default(),
            callback_handler: Default::default(),
            context_builder: Default::default(),
            context_assigner: Default::default(),
        }
    }
}

impl<P: IntoScriptPluginParams> Plugin for ScriptingPlugin<P> {
    fn build(&self, app: &mut bevy::prelude::App) {
        app.add_event::<ScriptErrorEvent>()
            .add_event::<ScriptCallbackEvent>()
            .init_resource::<AppReflectAllocator>()
            .init_resource::<ScriptAssetSettings>()
            .init_resource::<Scripts>()
            .init_resource::<AssetIdToScriptIdMap>()
            .init_asset::<ScriptAsset>()
            .register_asset_loader(ScriptAssetLoader {
                language: "<>".into(),
                extensions: &[],
                preprocessor: None,
            })
            .insert_resource(self.runtime_settings.as_ref().cloned().unwrap_or_default())
            .init_resource::<AppScriptFunctionRegistry>()
            .insert_non_send_resource::<RuntimeContainer<P>>(RuntimeContainer {
                runtime: (self.runtime_builder)(),
            })
            .init_non_send_resource::<ScriptContexts<P>>()
            .insert_resource::<CallbackSettings<P>>(CallbackSettings {
                callback_handler: self.callback_handler,
            })
            .insert_resource::<ContextLoadingSettings<P>>(ContextLoadingSettings {
                loader: self.context_builder.clone(),
                assigner: Some(self.context_assigner.clone().unwrap_or_default()),
                context_initializers: vec![],
                context_pre_handling_initializers: vec![],
            })
            .add_systems(PostUpdate, (garbage_collector, sync_script_data::<P>))
            .add_systems(PostStartup, initialize_runtime::<P>);

        register_types(app);
    }
}

/// Register all types that need to be accessed via reflection
fn register_types(app: &mut App) {
    app.register_type::<WorldCallbackAccess>();
    app.register_type::<ScriptValue>();
    app.register_type::<ScriptTypeRegistration>();
    app.register_type::<ReflectReference>();
}

pub trait AddRuntimeInitializer {
    fn add_runtime_initializer<P: IntoScriptPluginParams>(
        &mut self,
        initializer: RuntimeInitializer<P>,
    ) -> &mut Self;
}

impl AddRuntimeInitializer for App {
    fn add_runtime_initializer<P: IntoScriptPluginParams>(
        &mut self,
        initializer: RuntimeInitializer<P>,
    ) -> &mut Self {
        if !self.world_mut().contains_resource::<RuntimeSettings<P>>() {
            self.world_mut().init_resource::<RuntimeSettings<P>>();
        }
        self.world_mut()
            .resource_mut::<RuntimeSettings<P>>()
            .as_mut()
            .initializers
            .push(initializer);
        self
    }
}

pub trait AddContextInitializer {
    fn add_context_initializer<P: IntoScriptPluginParams>(
        &mut self,
        initializer: ContextInitializer<P>,
    ) -> &mut Self;
}

impl AddContextInitializer for App {
    fn add_context_initializer<P: IntoScriptPluginParams>(
        &mut self,
        initializer: ContextInitializer<P>,
    ) -> &mut Self {
        self.world_mut()
            .init_resource::<ContextLoadingSettings<P>>();
        self.world_mut()
            .resource_mut::<ContextLoadingSettings<P>>()
            .as_mut()
            .context_initializers
            .push(initializer);
        self
    }
}

pub trait AddContextPreHandlingInitializer {
    fn add_context_pre_handling_initializer<P: IntoScriptPluginParams>(
        &mut self,
        initializer: ContextPreHandlingInitializer<P>,
    ) -> &mut Self;
}

impl AddContextPreHandlingInitializer for App {
    fn add_context_pre_handling_initializer<P: IntoScriptPluginParams>(
        &mut self,
        initializer: ContextPreHandlingInitializer<P>,
    ) -> &mut Self {
        self.world_mut()
            .resource_mut::<ContextLoadingSettings<P>>()
            .as_mut()
            .context_pre_handling_initializers
            .push(initializer);
        self
    }
}

pub trait StoreDocumentation<D: DocumentationFragment> {
    /// Adds a documentation fragment to the documentation store.
    fn add_documentation_fragment(&mut self, fragment: D) -> &mut Self;
    /// Consumes all the stored documentation fragments, and merges them into one, then generates the documentation.
    fn generate_docs(&mut self) -> Result<(), Box<dyn std::error::Error>>;
}

impl<D: DocumentationFragment> StoreDocumentation<D> for App {
    fn add_documentation_fragment(&mut self, fragment: D) -> &mut Self {
        self.world_mut()
            .init_non_send_resource::<Documentation<D>>();
        self.world_mut()
            .non_send_resource_mut::<Documentation<D>>()
            .as_mut()
            .fragments
            .push(fragment);
        self
    }

    fn generate_docs(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let mut docs = match self
            .world_mut()
            .remove_non_send_resource::<Documentation<D>>()
        {
            Some(docs) => docs,
            None => return Ok(()),
        };

        let mut top_fragment = match docs.fragments.pop() {
            Some(fragment) => fragment,
            None => return Ok(()),
        };

        for fragment in docs.fragments.into_iter() {
            top_fragment = top_fragment.merge(fragment);
        }

        top_fragment.gen_docs()
    }
}

#[cfg(test)]
mod test {
    use asset::AssetIdToScriptIdMap;

    use super::*;

    #[test]
    fn test_default_scripting_plugin_initializes_all_resources_correctly() {
        let mut app = App::new();

        #[derive(Default, Clone)]
        struct C;
        #[derive(Default, Clone)]
        struct R;

        struct Plugin;

        impl IntoScriptPluginParams for Plugin {
            type C = C;
            type R = R;
        }

        app.add_plugins(AssetPlugin::default());
        app.add_plugins(ScriptingPlugin::<Plugin>::default());

        assert!(app.world().contains_resource::<Scripts>());
        assert!(app.world().contains_resource::<AppTypeRegistry>());
        assert!(app.world().contains_resource::<ScriptAssetSettings>());
        assert!(app.world().contains_resource::<RuntimeSettings<Plugin>>());
        assert!(app.world().contains_resource::<CallbackSettings<Plugin>>());
        assert!(app
            .world()
            .contains_resource::<ContextLoadingSettings<Plugin>>());
        assert!(app.world().contains_non_send::<RuntimeContainer<Plugin>>());
        assert!(app.world().contains_non_send::<ScriptContexts<Plugin>>());
        assert!(app.world().contains_resource::<AssetIdToScriptIdMap>());
    }
}
