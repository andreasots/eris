use crate::aiomas::Handler;
use crate::context::ErisContext;

pub struct AiomasHandler {
    pub method: &'static str,
    pub handler: &'static (dyn Handler<ErisContext> + Send + Sync + 'static),
}

impl AiomasHandler {
    pub fn new(
        method: &'static str,
        handler: &'static (dyn Handler<ErisContext> + Send + Sync + 'static),
    ) -> AiomasHandler {
        AiomasHandler { method, handler }
    }
}

inventory::collect!(AiomasHandler);
