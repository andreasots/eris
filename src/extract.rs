use failure::Error;
use serenity::prelude::*;
use std::any::type_name;

pub trait Extract {
    fn extract<T>(&self) -> Result<&T::Value, Error>
    where
        T: TypeMapKey,
        T::Value: Send + Sync;
    fn extract_mut<T>(&mut self) -> Result<&mut T::Value, Error>
    where
        T: TypeMapKey,
        T::Value: Send + Sync;
}

impl Extract for ShareMap {
    fn extract<T>(&self) -> Result<&T::Value, Error>
    where
        T: TypeMapKey,
        T::Value: Send + Sync,
    {
        self.get::<T>()
            .ok_or_else(|| failure::err_msg(format!("{} not in the sharemap", type_name::<T>())))
    }

    fn extract_mut<T>(&mut self) -> Result<&mut T::Value, Error>
    where
        T: TypeMapKey,
        T::Value: Send + Sync,
    {
        self.get_mut::<T>()
            .ok_or_else(|| failure::err_msg(format!("{} not in the sharemap", type_name::<T>())))
    }
}
