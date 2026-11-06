use super::global::Delivery;

impl Delivery {
    /// Whether this delivery reaches a channel with `only_when_away` set as
    /// given: an explicit answer always does, a presence one only while the
    /// user is away or when the channel does not wait for absence.
    pub(crate) fn reaches(self, only_when_away: bool) -> bool {
        match self {
            Delivery::Explicit => true,
            Delivery::Presence { away } => away || !only_when_away,
        }
    }
}

pub(crate) fn remote_delivery(solicited: bool, away: bool) -> Delivery {
    if solicited {
        Delivery::Explicit
    } else {
        Delivery::Presence { away }
    }
}

#[cfg(test)]
mod tests;
