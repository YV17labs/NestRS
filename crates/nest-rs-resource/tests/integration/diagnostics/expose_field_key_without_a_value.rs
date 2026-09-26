//! A field's valued keys written bare are refused naming the key, as the
//! decorator's own keys are — `via` and `complexity` reached `value()` and died
//! on syn's `` expected `=` ``, which names the grammar and not the key.

mod via_bare {
    use nest_rs_resource::expose;

    #[expose(name = "Thing")]
    pub struct Model {
        #[expose(via)]
        pub name: String,
    }
}

mod complexity_bare {
    use nest_rs_resource::expose;

    #[expose(name = "Thing")]
    pub struct Model {
        #[expose(complexity)]
        pub name: String,
    }
}

fn main() {}
