//! `reign-engine`: the Reign editor for projects without code of their own.
//! A project with code builds its own editor instead (see `reign_build`).

fn main() {
    reign_engine::run(reign_engine::Scripts::none());
}
