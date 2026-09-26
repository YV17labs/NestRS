//! A queue name reaches a Redis key, a metric label and a log field unescaped,
//! so the rule is checked at the literal — `:` is a key separator.

use nest_rs_queue::queue;

#[queue(name = "billing:invoices", job = String)]
struct InvoiceQueue;

fn main() {}
