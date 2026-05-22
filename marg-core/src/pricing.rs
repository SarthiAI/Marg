use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct ModelPrice {
    pub input_per_1k_usd: f64,
    pub output_per_1k_usd: f64,
}

#[derive(Debug, Clone, Default)]
pub struct PricingTable {
    by_model: HashMap<String, ModelPrice>,
}

impl PricingTable {
    pub fn new() -> Self {
        Self { by_model: HashMap::new() }
    }

    pub fn defaults_openai() -> Self {
        let mut t = Self::new();
        // Pricing as of early 2026, USD per 1K tokens. Operators override in config.
        t.insert("gpt-4o", ModelPrice { input_per_1k_usd: 0.0025, output_per_1k_usd: 0.010 });
        t.insert("gpt-4o-mini", ModelPrice { input_per_1k_usd: 0.00015, output_per_1k_usd: 0.0006 });
        t.insert("gpt-4-turbo", ModelPrice { input_per_1k_usd: 0.01, output_per_1k_usd: 0.03 });
        t.insert("gpt-4", ModelPrice { input_per_1k_usd: 0.03, output_per_1k_usd: 0.06 });
        t.insert("gpt-3.5-turbo", ModelPrice { input_per_1k_usd: 0.0005, output_per_1k_usd: 0.0015 });
        t
    }

    pub fn insert(&mut self, model: &str, price: ModelPrice) {
        self.by_model.insert(model.to_string(), price);
    }

    pub fn extend_from_entries<I, S>(&mut self, entries: I)
    where
        I: IntoIterator<Item = (S, ModelPrice)>,
        S: Into<String>,
    {
        for (model, price) in entries {
            self.by_model.insert(model.into(), price);
        }
    }

    pub fn lookup(&self, model: &str) -> Option<ModelPrice> {
        self.by_model.get(model).copied()
    }

    pub fn cost_usd(&self, model: &str, input_tokens: u64, output_tokens: u64) -> f64 {
        let Some(price) = self.lookup(model) else { return 0.0 };
        let input_cost = (input_tokens as f64 / 1000.0) * price.input_per_1k_usd;
        let output_cost = (output_tokens as f64 / 1000.0) * price.output_per_1k_usd;
        input_cost + output_cost
    }

    pub fn known_models(&self) -> impl Iterator<Item = &String> {
        self.by_model.keys()
    }
}
