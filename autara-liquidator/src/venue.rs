#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Venue {
    Clamm,
    PropAmm,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VenueQuote<T> {
    pub venue: Venue,
    pub amount_in: u64,
    pub estimated_out: u64,
    pub execution: T,
}

impl<T> VenueQuote<T> {
    pub fn map_execution<U>(self, map: impl FnOnce(T) -> U) -> VenueQuote<U> {
        VenueQuote {
            venue: self.venue,
            amount_in: self.amount_in,
            estimated_out: self.estimated_out,
            execution: map(self.execution),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Venue, VenueQuote};

    #[test]
    fn maps_execution_without_changing_quote_metadata() {
        let quote = VenueQuote {
            venue: Venue::Clamm,
            amount_in: 10,
            estimated_out: 20,
            execution: "callback",
        };

        let mapped = quote.map_execution(str::len);

        assert_eq!(mapped.venue, Venue::Clamm);
        assert_eq!(mapped.amount_in, 10);
        assert_eq!(mapped.estimated_out, 20);
        assert_eq!(mapped.execution, 8);
    }
}
