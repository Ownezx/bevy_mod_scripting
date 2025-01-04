use crate::{
    asset::{AssetIdToScriptIdMap, ScriptAsset, ScriptAssetSettings},
    bindings::{pretty_print::DisplayWithWorld, AppReflectAllocator, WorldAccessGuard, WorldGuard},
    commands::{CreateOrUpdateScript, DeleteScript},
    context::{ContextLoadingSettings, ScriptContexts},
    error::ScriptError,
    event::{IntoCallbackLabel, ScriptCallbackEvent, ScriptErrorEvent},
    handler::CallbackSettings,
    runtime::{RuntimeContainer, RuntimeSettings},
    script::{ScriptComponent, Scripts},
    IntoScriptPluginParams,
};
use bevy::{ecs::system::SystemState, prelude::*};
use std::any::type_name;

/// Cleans up dangling script allocations
pub fn garbage_collector(allocator: ResMut<AppReflectAllocator>) {
    let mut allocator = allocator.write();
    allocator.clean_garbage_allocations()
}

pub fn initialize_runtime<P: IntoScriptPluginParams>(
    mut runtime: NonSendMut<RuntimeContainer<P>>,
    settings: Res<RuntimeSettings<P>>,
) {
    for initializer in settings.initializers.iter() {
        (initializer)(&mut runtime.runtime);
    }
}

/// Processes and reacts appropriately to script asset events, and queues commands to update the internal script state
pub fn sync_script_data<P: IntoScriptPluginParams>(
    mut events: EventReader<AssetEvent<ScriptAsset>>,
    script_assets: Res<Assets<ScriptAsset>>,
    asset_settings: Res<ScriptAssetSettings>,
    mut asset_path_map: ResMut<AssetIdToScriptIdMap>,
    mut commands: Commands,
) {
    for event in events.read() {
        trace!("Received script asset event: {:?}", event);
        let (id, remove) = match event {
            // emitted when a new script asset is loaded for the first time
            AssetEvent::Added { id } => (id, false),
            AssetEvent::Modified { id } => (id, false),
            AssetEvent::Removed { id } => (id, true),
            _ => continue,
        };
        info!("Responding to script asset event: {:?}", event);
        // get the path
        let asset = script_assets.get(*id);

        let script_id = match asset_path_map.get(*id) {
            Some(id) => id.clone(),
            None => {
                // we should only enter this branch for new assets
                let asset = match asset {
                    Some(asset) => asset,
                    None => {
                        // this can happen if an asset is loaded and immediately unloaded, we can ignore this
                        continue;
                    }
                };

                let path = &asset.asset_path;
                let converter = asset_settings.script_id_mapper.map;
                let script_id = converter(path);
                asset_path_map.insert(*id, script_id.clone());

                script_id
            }
        };

        if !remove {
            let asset = match asset {
                Some(asset) => asset,
                None => {
                    // this can happen if an asset is loaded and immediately unloaded, we can ignore this
                    continue;
                }
            };
            info!("Creating or updating script with id: {}", script_id);
            commands.queue(CreateOrUpdateScript::<P>::new(
                script_id,
                asset.content.clone(),
                Some(script_assets.reserve_handle().clone_weak()),
            ));
        } else {
            commands.queue(DeleteScript::<P>::new(script_id));
        }
    }
}

macro_rules! push_err_and_continue {
    ($errors:ident, $expr:expr) => {
        match $expr {
            Ok(v) => v,
            Err(e) => {
                $errors.push(e);
                continue;
            }
        }
    };
}

/// Passes events with the specified label to the script callback with the same name and runs the callback
pub fn event_handler<L: IntoCallbackLabel, P: IntoScriptPluginParams>(
    world: &mut World,
    params: &mut SystemState<(
        EventReader<ScriptCallbackEvent>,
        Res<CallbackSettings<P>>,
        Res<ContextLoadingSettings<P>>,
        Res<Scripts>,
        Query<(Entity, Ref<ScriptComponent>)>,
    )>,
) {
    trace!("Handling events with label `{}`", L::into_callback_label());

    let mut runtime_container = world
        .remove_non_send_resource::<RuntimeContainer<P>>()
        .unwrap_or_else(|| {
            panic!(
                "No runtime container for runtime {} found. Was the scripting plugin initialized correctly?",
                type_name::<P::R>()
            )
        });
    let runtime = &mut runtime_container.runtime;
    let mut script_contexts = world
        .remove_non_send_resource::<ScriptContexts<P>>()
        .unwrap_or_else(|| panic!("No script contexts found for context {}", type_name::<P>()));

    let (mut script_events, callback_settings, context_settings, scripts, entities) =
        params.get_mut(world);

    let handler = *callback_settings
        .callback_handler
        .as_ref()
        .unwrap_or_else(|| {
            panic!(
                "No handler registered for - Runtime: {}, Context: {}",
                type_name::<P::R>(),
                type_name::<P::C>()
            )
        });
    let pre_handling_initializers = context_settings.context_pre_handling_initializers.clone();
    let scripts = scripts.clone();
    let mut errors = Vec::default();

    let events = script_events.read().cloned().collect::<Vec<_>>();
    let entity_scripts = entities
        .iter()
        .map(|(e, s)| (e, s.0.clone()))
        .collect::<Vec<_>>();

    for event in events
        .into_iter()
        .filter(|e| e.label == L::into_callback_label())
    {
        for (entity, entity_scripts) in entity_scripts.iter() {
            for script_id in entity_scripts.iter() {
                match &event.recipients {
                    crate::event::Recipients::Script(target_script_id)
                        if target_script_id != script_id =>
                    {
                        continue
                    }
                    crate::event::Recipients::Entity(target_entity) if target_entity != entity => {
                        continue
                    }
                    _ => (),
                }
                debug!(
                    "Handling event for script {} on entity {:?}",
                    script_id, entity
                );
                let script = match scripts.scripts.get(script_id) {
                    Some(s) => s,
                    None => {
                        trace!(
                            "Script `{}` on entity `{:?}` is either still loading or doesn't exist, ignoring.",
                            script_id, entity
                        );
                        continue;
                    }
                };
                let ctxt = script_contexts
                    .contexts
                    .get_mut(&script.context_id)
                    .unwrap();

                let handler_result = (handler)(
                    event.args.clone(),
                    *entity,
                    &script.id,
                    &L::into_callback_label(),
                    ctxt,
                    &pre_handling_initializers,
                    runtime,
                    world,
                )
                .map_err(|e| {
                    e.with_script(script.id.clone()).with_context(format!(
                        "Event handling for: Runtime {}, Context: {}",
                        type_name::<P::R>(),
                        type_name::<P::C>(),
                    ))
                });

                push_err_and_continue!(errors, handler_result)
            }
        }
    }

    world.insert_non_send_resource(runtime_container);
    world.insert_non_send_resource(script_contexts);

    handle_script_errors(world, errors.into_iter());
}

/// Handles errors caused by script execution and sends them to the error event channel
pub(crate) fn handle_script_errors<I: Iterator<Item = ScriptError> + Clone>(
    world: &mut World,
    errors: I,
) {
    let mut error_events = world
        .get_resource_mut::<Events<ScriptErrorEvent>>()
        .expect("Missing events resource");

    for error in errors.clone() {
        error_events.send(ScriptErrorEvent { error });
    }

    for error in errors {
        let arc_world = WorldGuard::new(WorldAccessGuard::new(world));
        bevy::log::error!("{}", error.display_with_world(arc_world));
    }
}

#[cfg(test)]
mod test {
    use std::{borrow::Cow, collections::HashMap};

    use crate::{
        bindings::script_value::ScriptValue,
        event::CallbackLabel,
        handler::HandlerFn,
        script::{Script, ScriptId},
    };

    use super::*;
    struct OnTestCallback;

    impl IntoCallbackLabel for OnTestCallback {
        fn into_callback_label() -> CallbackLabel {
            "OnTest".into()
        }
    }

    struct TestPlugin;

    impl IntoScriptPluginParams for TestPlugin {
        type C = TestContext;
        type R = TestRuntime;
    }

    struct TestRuntime {
        pub invocations: Vec<(Entity, ScriptId)>,
    }

    struct TestContext {
        pub invocations: Vec<ScriptValue>,
    }

    fn setup_app<L: IntoCallbackLabel + 'static, P: IntoScriptPluginParams>(
        handler_fn: HandlerFn<P>,
        runtime: P::R,
        contexts: HashMap<u32, P::C>,
        scripts: HashMap<ScriptId, Script>,
    ) -> App {
        let mut app = App::new();

        app.add_event::<ScriptCallbackEvent>();
        app.add_event::<ScriptErrorEvent>();
        app.insert_resource::<CallbackSettings<P>>(CallbackSettings {
            callback_handler: Some(handler_fn),
        });
        app.add_systems(Update, event_handler::<L, P>);
        app.insert_resource::<Scripts>(Scripts { scripts });
        app.insert_non_send_resource(RuntimeContainer::<P> { runtime });
        app.insert_non_send_resource(ScriptContexts::<P> { contexts });
        app.insert_resource(ContextLoadingSettings::<P> {
            loader: None,
            assigner: None,
            context_initializers: vec![],
            context_pre_handling_initializers: vec![],
        });
        app.finish();
        app.cleanup();
        app
    }

    #[test]
    fn test_handler_called_with_right_args() {
        let test_script_id = Cow::Borrowed("test_script");
        let test_ctxt_id = 0;
        let test_script = Script {
            id: test_script_id.clone(),
            asset: None,
            context_id: test_ctxt_id,
        };
        let scripts = HashMap::from_iter(vec![(test_script_id.clone(), test_script.clone())]);
        let contexts = HashMap::from_iter(vec![(
            test_ctxt_id,
            TestContext {
                invocations: vec![],
            },
        )]);
        let runtime = TestRuntime {
            invocations: vec![],
        };
        let mut app = setup_app::<OnTestCallback, TestPlugin>(
            |args, entity, script, _, ctxt, _, runtime, _| {
                ctxt.invocations.extend(args);
                runtime.invocations.push((entity, script.clone()));
                Ok(())
            },
            runtime,
            contexts,
            scripts,
        );
        let test_entity_id = app
            .world_mut()
            .spawn(ScriptComponent(vec![test_script_id.clone()]))
            .id();

        app.world_mut().send_event(ScriptCallbackEvent::new_for_all(
            OnTestCallback::into_callback_label(),
            vec![ScriptValue::String("test_args".into())],
        ));
        app.update();

        let test_context = app
            .world()
            .get_non_send_resource::<ScriptContexts<TestPlugin>>()
            .unwrap();
        let test_runtime = app
            .world()
            .get_non_send_resource::<RuntimeContainer<TestPlugin>>()
            .unwrap();

        assert_eq!(
            test_context
                .contexts
                .get(&test_ctxt_id)
                .unwrap()
                .invocations,
            vec![ScriptValue::String("test_args".into())]
        );

        assert_eq!(
            test_runtime
                .runtime
                .invocations
                .iter()
                .map(|(e, s)| (*e, s.clone()))
                .collect::<Vec<_>>(),
            vec![(test_entity_id, test_script_id.clone())]
        );
    }

    #[test]
    fn test_handler_called_on_right_recipients() {
        let test_script_id = Cow::Borrowed("test_script");
        let test_ctxt_id = 0;
        let test_script = Script {
            id: test_script_id.clone(),
            asset: None,
            context_id: test_ctxt_id,
        };
        let scripts = HashMap::from_iter(vec![
            (test_script_id.clone(), test_script.clone()),
            (
                "wrong".into(),
                Script {
                    id: "wrong".into(),
                    asset: None,
                    context_id: 1,
                },
            ),
        ]);
        let contexts = HashMap::from_iter(vec![
            (
                test_ctxt_id,
                TestContext {
                    invocations: vec![],
                },
            ),
            (
                1,
                TestContext {
                    invocations: vec![],
                },
            ),
        ]);
        let runtime = TestRuntime {
            invocations: vec![],
        };
        let mut app = setup_app::<OnTestCallback, TestPlugin>(
            |args, entity, script, _, ctxt, _, runtime, _| {
                ctxt.invocations.extend(args);
                runtime.invocations.push((entity, script.clone()));
                Ok(())
            },
            runtime,
            contexts,
            scripts,
        );
        let test_entity_id = app
            .world_mut()
            .spawn(ScriptComponent(vec![test_script_id.clone()]))
            .id();

        app.world_mut().send_event(ScriptCallbackEvent::new(
            OnTestCallback::into_callback_label(),
            vec![ScriptValue::String("test_args_script".into())],
            crate::event::Recipients::Script(test_script_id.clone()),
        ));

        app.world_mut().send_event(ScriptCallbackEvent::new(
            OnTestCallback::into_callback_label(),
            vec![ScriptValue::String("test_args_entity".into())],
            crate::event::Recipients::Entity(test_entity_id),
        ));

        app.update();

        let test_context = app
            .world()
            .get_non_send_resource::<ScriptContexts<TestPlugin>>()
            .unwrap();
        let test_runtime = app
            .world()
            .get_non_send_resource::<RuntimeContainer<TestPlugin>>()
            .unwrap();

        assert_eq!(
            test_context
                .contexts
                .get(&test_ctxt_id)
                .unwrap()
                .invocations,
            vec![
                ScriptValue::String("test_args_script".into()),
                ScriptValue::String("test_args_entity".into())
            ]
        );

        assert_eq!(
            test_runtime
                .runtime
                .invocations
                .iter()
                .map(|(e, s)| (*e, s.clone()))
                .collect::<Vec<_>>(),
            vec![
                (test_entity_id, test_script_id.clone()),
                (test_entity_id, test_script_id.clone())
            ]
        );
    }
}
