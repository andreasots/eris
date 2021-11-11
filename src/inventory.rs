use crate::aiomas::Handler;
use crate::context::ErisContext;

pub struct AiomasHandler {
    pub method: &'static str,
    pub handler: &'static (dyn Handler<ErisContext> + Send + Sync + 'static),
}

inventory::collect!(AiomasHandler);
