use std::borrow::Cow;
use std::cell::RefCell;
use std::sync::LazyLock;

use anyhow::{Context, Error};
use chrono::{DateTime, Utc};
use html5ever::interface::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::{Attribute, LocalName, QualName};
use reqwest::Client;
use serde::Deserialize;

#[derive(Deserialize)]
struct HeaderProps {
    #[serde(rename = "currentEvent")]
    current_event: (f64, Event),
}

#[derive(Deserialize)]
struct Event {
    total: (f64, f64),
    #[serde(rename = "startsAt")]
    starts_at: (f64, DateTime<Utc>),
}

struct HeaderSink {
    header: RefCell<Option<StrTendril>>,
}

impl HeaderSink {
    fn new() -> Self {
        Self { header: RefCell::new(None) }
    }
}

impl TreeSink for HeaderSink {
    type Handle = Option<QualName>;

    type Output = Option<StrTendril>;

    type ElemName<'a> = &'a QualName;

    fn finish(self) -> Self::Output {
        self.header.borrow_mut().take()
    }

    fn parse_error(&self, msg: Cow<'static, str>) {
        tracing::error!("HTML parse error: {msg}")
    }

    fn get_document(&self) -> Self::Handle {
        None
    }

    fn elem_name<'a>(&'a self, target: &'a Self::Handle) -> Self::ElemName<'a> {
        target.as_ref().unwrap()
    }

    fn create_element(
        &self,
        name: QualName,
        attrs: Vec<Attribute>,
        _flags: ElementFlags,
    ) -> Self::Handle {
        static HEADER_ELEMENT_NAME: LazyLock<QualName> = LazyLock::new(|| {
            QualName::new(None, html5ever::ns!(html), LocalName::from("astro-island"))
        });
        static HEADER_COMPONENT_ATTR_NAME: LazyLock<QualName> = LazyLock::new(|| {
            QualName::new(None, html5ever::ns!(), LocalName::from("component-export"))
        });
        const HEADER_COMPONENT_NAME: &str = "Header";
        static HEADER_VALUE_ATTR_NAME: LazyLock<QualName> =
            LazyLock::new(|| QualName::new(None, html5ever::ns!(), LocalName::from("props")));

        if name.expanded() == HEADER_ELEMENT_NAME.expanded()
            && attrs.iter().any(|attr| {
                attr.name.expanded() == HEADER_COMPONENT_ATTR_NAME.expanded()
                    && &*attr.value == HEADER_COMPONENT_NAME
            })
        {
            *self.header.borrow_mut() = attrs
                .iter()
                .find(|attr| attr.name.expanded() == HEADER_VALUE_ATTR_NAME.expanded())
                .map(|attr| attr.value.clone());
        }

        Some(name)
    }

    fn create_comment(&self, _text: StrTendril) -> Self::Handle {
        None
    }

    fn create_pi(&self, _target: StrTendril, _data: StrTendril) -> Self::Handle {
        None
    }

    fn append(&self, _parent: &Self::Handle, _child: NodeOrText<Self::Handle>) {}

    fn append_based_on_parent_node(
        &self,
        _element: &Self::Handle,
        _prev_element: &Self::Handle,
        _child: NodeOrText<Self::Handle>,
    ) {
    }

    fn append_doctype_to_document(
        &self,
        _name: StrTendril,
        _public_id: StrTendril,
        _system_id: StrTendril,
    ) {
    }

    fn get_template_contents(&self, _target: &Self::Handle) -> Self::Handle {
        None
    }

    fn same_node(&self, x: &Self::Handle, y: &Self::Handle) -> bool {
        x == y
    }

    fn set_quirks_mode(&self, _mode: QuirksMode) {}

    fn append_before_sibling(&self, _sibling: &Self::Handle, _new_node: NodeOrText<Self::Handle>) {}

    fn add_attrs_if_missing(&self, _target: &Self::Handle, _attrs: Vec<Attribute>) {}

    fn remove_from_parent(&self, _target: &Self::Handle) {}

    fn reparent_children(&self, _node: &Self::Handle, _new_parent: &Self::Handle) {}
}

#[derive(Clone)]
pub struct DesertBus {
    client: Client,
}

impl DesertBus {
    pub const FIRST_HOUR: f64 = 1.00;
    pub const MULTIPLIER: f64 = 1.07;

    pub fn new(client: Client) -> DesertBus {
        DesertBus { client }
    }

    pub fn hours_raised(money_raised: f64) -> f64 {
        // money_raised = FIRST_HOUR + FIRST_HOUR * MULTIPLIER + FIRST_HOUR * MULTIPLIER.pow(2.0) + ... + FIRST_HOUR * MULTIPLIER.pow(hours)
        // money_raised = FIRST_HOUR * (1.0 - MULTIPLIER.pow(hours)) / (1.0 - MULTIPLIER)
        // money_raised / FIRST_HOUR = (MULTIPLIER.pow(hours) - 1.0) / (MULTIPLIER - 1.0)
        // money_raised / FIRST_HOUR * (MULTIPLIER - 1.0) = MULTIPLIER.pow(hours) - 1.0
        // MULTIPLIER.pow(hours) = money_raised / FIRST_HOUR * (MULTIPLIER - 1.0) + 1.0
        // hours = (money_raised / FIRST_HOUR * (MULTIPLIER - 1.0) + 1.0).log(MULTIPLIER)

        (money_raised / DesertBus::FIRST_HOUR * (DesertBus::MULTIPLIER - 1.0) + 1.0)
            .log(DesertBus::MULTIPLIER)
            .floor()
    }

    pub async fn fetch_current_event(&self) -> Result<(DateTime<Utc>, f64), Error> {
        let html = self
            .client
            .get("https://desertbus.org/")
            .send()
            .await
            .context("failed to request the Desert Bus homepage")?
            .text()
            .await
            .context("failed to read the Desert Bus homepage")?;

        let header = html5ever::parse_document(HeaderSink::new(), Default::default())
            .one(html)
            .context("failed to find the header component")?;

        let props =
            serde_json::from_str::<HeaderProps>(&header).context("failed to parse header props")?;

        return Ok((props.current_event.1.starts_at.1, props.current_event.1.total.1));
    }
}
