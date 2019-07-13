use failure::Error;
use std::any::Any;
use std::borrow::Cow;

fn downcast_panic_message(msg: Box<dyn Any + 'static>) -> Cow<'static, str> {
    msg.downcast::<&'static str>()
        .map(|s| Cow::Borrowed(*s))
        .or_else(|msg| msg.downcast::<String>().map(|s| Cow::Owned(*s)))
        .unwrap_or_else(|_| Cow::Borrowed("<unknown type>"))
}

pub fn run<T: Send, F: Send + FnOnce() -> Result<T, Error>>(f: F) -> Result<T, Error> {
    let res = crossbeam::scope(|scope| {
        match scope.spawn(|_| f()).join() {
            Ok(res) => res,
            Err(err) => return Err(failure::err_msg(format!("closure panicked: {:?}", downcast_panic_message(err)))),
        }
    });
    match res {
        Ok(res) => res,
        Err(err) => Err(failure::err_msg(format!("unjoined thread panicked: {:?}", downcast_panic_message(err)))),
    }
}
