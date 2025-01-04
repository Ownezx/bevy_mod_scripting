use std::{
    any::TypeId,
    ops::{Deref, DerefMut},
};

use bevy_mod_scripting_core::bindings::{
    function::{script_function::CallerContext, CallScriptFunction},
    script_value::ScriptValue,
};
use mlua::{FromLua, IntoLua, Value, Variadic};

use super::{reference::LuaReflectReference, world::GetWorld};

#[derive(Debug, Clone)]
pub struct LuaScriptValue(ScriptValue);

impl Deref for LuaScriptValue {
    type Target = ScriptValue;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for LuaScriptValue {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl From<ScriptValue> for LuaScriptValue {
    fn from(value: ScriptValue) -> Self {
        LuaScriptValue(value)
    }
}

impl From<LuaScriptValue> for ScriptValue {
    fn from(value: LuaScriptValue) -> Self {
        value.0
    }
}

impl FromLua for LuaScriptValue {
    fn from_lua(value: mlua::Value, _lua: &mlua::Lua) -> mlua::Result<Self> {
        Ok(match value {
            Value::Nil => ScriptValue::Unit,
            Value::Boolean(b) => ScriptValue::Bool(b),
            // Value::LightUserData(light_user_data) => todo!(),
            Value::Integer(i) => ScriptValue::Integer(i),
            Value::Number(n) => ScriptValue::Float(n),
            Value::String(s) => ScriptValue::String(s.to_str()?.to_owned().into()),
            Value::Table(table) => {
                let mut vec = Vec::with_capacity(table.len()? as usize);
                for i in table.sequence_values() {
                    let v: LuaScriptValue = i?;
                    vec.push(v.into());
                }
                ScriptValue::List(vec)
            }
            // Value::Function(function) => todo!(),
            // Value::Thread(thread) => todo!(),
            Value::UserData(ud) => {
                let ud = ud.borrow::<LuaReflectReference>().map_err(|e| {
                    mlua::Error::FromLuaConversionError {
                        from: "UserData",
                        to: "LuaReflectReference".to_owned(),
                        message: Some(e.to_string()),
                    }
                })?;
                ScriptValue::Reference(ud.clone().into())
            }
            // Value::Error(error) => todo!(),
            _ => {
                return Err(mlua::Error::FromLuaConversionError {
                    from: value.type_name(),
                    to: "ScriptValue".to_owned(),
                    message: Some("unsupported value type".to_owned()),
                })
            }
        }
        .into())
    }
}

pub fn lua_caller_context(self_type: Option<TypeId>) -> CallerContext {
    CallerContext {
        convert_to_0_indexed: true,
        self_type,
    }
}

impl IntoLua for LuaScriptValue {
    fn into_lua(self, lua: &mlua::Lua) -> mlua::Result<mlua::Value> {
        Ok(match self.0 {
            ScriptValue::Unit => Value::Nil,
            ScriptValue::Bool(b) => Value::Boolean(b),
            ScriptValue::Integer(i) => Value::Integer(i),
            ScriptValue::Float(f) => Value::Number(f),
            ScriptValue::String(s) => Value::String(lua.create_string(s.as_ref())?),
            ScriptValue::Reference(r) => LuaReflectReference::from(r).into_lua(lua)?,
            ScriptValue::Error(script_error) => return Err(mlua::Error::external(script_error)),
            ScriptValue::Function(mut function) => lua
                .create_function_mut(move |lua, args: Variadic<LuaScriptValue>| {
                    let world = lua.get_world();
                    let out = function.call_script_function(
                        args.into_iter().map(Into::into),
                        world,
                        lua_caller_context(None),
                    )?;

                    Ok(LuaScriptValue::from(out))
                })?
                .into_lua(lua)?,
            ScriptValue::List(vec) => {
                let table = lua.create_table_from(
                    vec.into_iter()
                        .enumerate()
                        .map(|(k, v)| (k + 1, LuaScriptValue::from(v))),
                )?;
                Value::Table(table)
            }
        })
    }
}
