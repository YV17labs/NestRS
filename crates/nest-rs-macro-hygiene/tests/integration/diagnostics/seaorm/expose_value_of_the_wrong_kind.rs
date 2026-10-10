//! `#[expose]`'s values, each of the wrong kind and each refused at itself,
//! opening with the decorator and the key: a name that is not a string, a
//! service that is not a path, `input` and `validate` given no list, a list
//! element that is not one of the two write DTOs, and a `via` that is not a
//! string. syn's `expected string literal` / `expected identifier` /
//! `expected parentheses` named none of them.

mod name_not_a_string {
    use nest_rs::seaorm::expose;

    #[expose(name = Thing)]
    pub struct Model {
        pub id: i32,
    }
}

mod service_not_a_path {
    use nest_rs::seaorm::expose;

    #[expose(name = "Thing", service = "ThingsService")]
    pub struct Model {
        pub id: i32,
    }
}

mod input_given_no_list {
    use nest_rs::seaorm::expose;

    #[expose(name = "Thing")]
    pub struct Model {
        #[expose(input = create)]
        pub name: String,
    }
}

mod input_element_not_a_dto {
    use nest_rs::seaorm::expose;

    #[expose(name = "Thing")]
    pub struct Model {
        #[expose(input("create"))]
        pub name: String,
    }
}

mod validate_given_no_list {
    use nest_rs::seaorm::expose;

    #[expose(name = "Thing")]
    pub struct Model {
        #[expose(validate)]
        pub name: String,
    }
}

mod via_not_a_string {
    use nest_rs::seaorm::expose;

    #[expose(name = "Thing")]
    pub struct Model {
        #[expose(via = author_id)]
        pub name: String,
    }
}

fn main() {}
