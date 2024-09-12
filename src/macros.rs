#[rustfmt::skip]
macro_rules! all_the_tuples {
    ($name:ident) => {
        $name!([], A);
        $name!([A], B);
        $name!([A, B], C);
        $name!([A, B, C], D);
        $name!([A, B, C, D], E);
        $name!([A, B, C, D, E], F);
        $name!([A, B, C, D, E, F], G);
        $name!([A, B, C, D, E, F, G], H);
        $name!([A, B, C, D, E, F, G, H], I);
        $name!([A, B, C, D, E, F, G, H, I], J);
        $name!([A, B, C, D, E, F, G, H, I, J], K);
        $name!([A, B, C, D, E, F, G, H, I, J, K], L);
        $name!([A, B, C, D, E, F, G, H, I, J, K, L], M);
        $name!([A, B, C, D, E, F, G, H, I, J, K, L, M], N);
        $name!([A, B, C, D, E, F, G, H, I, J, K, L, M, N], O);
        $name!([A, B, C, D, E, F, G, H, I, J, K, L, M, N, O], P);
    };
}

macro_rules! all_the_tuples_no_last_special_case {
    ($name:ident) => {
        $name!(A);
        $name!(A, B);
        $name!(A, B, C);
        $name!(A, B, C, D);
        $name!(A, B, C, D, E);
        $name!(A, B, C, D, E, F);
        $name!(A, B, C, D, E, F, G);
        $name!(A, B, C, D, E, F, G, H);
        $name!(A, B, C, D, E, F, G, H, I);
        $name!(A, B, C, D, E, F, G, H, I, J);
        $name!(A, B, C, D, E, F, G, H, I, J, K);
        $name!(A, B, C, D, E, F, G, H, I, J, K, L);
        $name!(A, B, C, D, E, F, G, H, I, J, K, L, M);
        $name!(A, B, C, D, E, F, G, H, I, J, K, L, M, N);
        $name!(A, B, C, D, E, F, G, H, I, J, K, L, M, N, O);
        $name!(A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P);
    };
}
