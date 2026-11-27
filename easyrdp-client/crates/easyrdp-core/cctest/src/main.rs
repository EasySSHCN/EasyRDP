extern "C"{fn add1(x: i32)->i32;} fn main(){println!("{}", unsafe{add1(1)});}
