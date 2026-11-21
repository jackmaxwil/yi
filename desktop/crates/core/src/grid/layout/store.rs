//! How the grid writes what it does not own to a file: an id as its number, a pane kind and
//! a saved layout as their slugs.

/// A pane id as its number.
pub(crate) mod pane {
    use serde::{Deserialize, Deserializer, Serializer};

    use crate::ids::PaneId;

    pub fn serialize<S: Serializer>(id: &PaneId, to: S) -> Result<S::Ok, S::Error> {
        to.serialize_u32(id.0)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(from: D) -> Result<PaneId, D::Error> {
        u32::deserialize(from).map(PaneId)
    }
}

/// Lists of pane ids, as lists of their numbers.
pub(crate) mod stacks {
    use serde::{Deserialize, Deserializer, Serializer};

    use crate::ids::PaneId;

    pub fn serialize<S: Serializer>(stacks: &[Vec<PaneId>], to: S) -> Result<S::Ok, S::Error> {
        let numbers = |stack: &Vec<PaneId>| stack.iter().map(|pane| pane.0).collect::<Vec<_>>();
        to.collect_seq(stacks.iter().map(numbers))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(from: D) -> Result<Vec<Vec<PaneId>>, D::Error> {
        let lists = Vec::<Vec<u32>>::deserialize(from)?;
        let ids = |list: Vec<u32>| list.into_iter().map(PaneId).collect();
        Ok(lists.into_iter().map(ids).collect())
    }
}

/// What a pane is about, as the text of its address; a pane about its kind alone has none.
pub(crate) mod thing {
    use serde::de::Error;
    use serde::{Deserialize, Deserializer, Serializer};

    use crate::address::uri::Address;

    pub fn serialize<S: Serializer>(thing: &Option<Address>, to: S) -> Result<S::Ok, S::Error> {
        to.collect_str(&thing.as_ref().map(ToString::to_string).unwrap_or_default())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(from: D) -> Result<Option<Address>, D::Error> {
        let text = String::deserialize(from)?;
        text.parse().map(Some).map_err(D::Error::custom)
    }
}

/// A window id as its number.
pub(crate) mod window {
    use serde::{Deserialize, Deserializer, Serializer};

    use crate::ids::WindowId;

    pub fn serialize<S: Serializer>(id: &WindowId, to: S) -> Result<S::Ok, S::Error> {
        to.serialize_u32(id.0)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(from: D) -> Result<WindowId, D::Error> {
        u32::deserialize(from).map(WindowId)
    }
}

/// A list of things each kept under a pane, as pairs of the pane's number and the thing.
pub(crate) mod keyed {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use crate::ids::PaneId;

    pub fn serialize<T: Serialize, S: Serializer>(
        list: &[(PaneId, T)],
        to: S,
    ) -> Result<S::Ok, S::Error> {
        to.collect_seq(list.iter().map(|(pane, value)| (pane.0, value)))
    }

    pub fn deserialize<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
        from: D,
    ) -> Result<Vec<(PaneId, T)>, D::Error> {
        let list = Vec::<(u32, T)>::deserialize(from)?;
        Ok(list
            .into_iter()
            .map(|(pane, value)| (PaneId(pane), value))
            .collect())
    }
}

/// A pane kind as its slug. A slug this build does not know refuses the file.
pub(crate) mod kind {
    use serde::de::Error;
    use serde::{Deserialize, Deserializer, Serializer};

    use crate::pane::PaneKind;

    pub fn serialize<S: Serializer>(kind: &PaneKind, to: S) -> Result<S::Ok, S::Error> {
        to.serialize_str(kind.slug())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(from: D) -> Result<PaneKind, D::Error> {
        let slug = String::deserialize(from)?;
        let found = PaneKind::ALL.iter().find(|kind| kind.slug() == slug);
        found
            .copied()
            .ok_or_else(|| D::Error::custom("a kind of pane this build does not have"))
    }
}

/// Slots kept by pane kind, as pairs of the kind's slug and the area.
pub(crate) mod kinds {
    use serde::{Deserialize, Deserializer, Serializer};

    use crate::grid::layout::Area;
    use crate::pane::PaneKind;

    pub fn serialize<S: Serializer>(list: &[(PaneKind, Area)], to: S) -> Result<S::Ok, S::Error> {
        to.collect_seq(list.iter().map(|(kind, area)| (kind.slug(), area)))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        from: D,
    ) -> Result<Vec<(PaneKind, Area)>, D::Error> {
        let list = Vec::<(String, Area)>::deserialize(from)?;
        let kind = |slug: &str| PaneKind::ALL.iter().copied().find(|k| k.slug() == slug);
        let read = list.into_iter().map(|(slug, area)| match kind(&slug) {
            Some(kind) => Ok((kind, area)),
            None => Err(serde::de::Error::custom(
                "a kind of pane this build does not have",
            )),
        });
        read.collect()
    }
}

/// The saved layout a window shows, as its slug. One this build lacks reads as none.
pub(crate) mod slug {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use crate::grid::saved::find;

    pub fn serialize<S: Serializer>(
        saved: &Option<&'static str>,
        to: S,
    ) -> Result<S::Ok, S::Error> {
        saved.serialize(to)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        from: D,
    ) -> Result<Option<&'static str>, D::Error> {
        let name = Option::<String>::deserialize(from)?;
        Ok(name.and_then(|name| find(&name)).map(|saved| saved.slug))
    }
}
