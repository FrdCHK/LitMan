use std::error::Error;
use std::path::PathBuf;

use litman_core::{Library, ListFilter};

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let config = PathBuf::from(arguments.next().ok_or("missing config path")?);
    let relative_path = arguments
        .next()
        .ok_or("missing selected relative path")?
        .into_string()
        .map_err(|_| "selected relative path must be Unicode")?;
    let publisher_pdf = PathBuf::from(arguments.next().ok_or("missing publisher PDF path")?);
    if arguments.next().is_some() {
        return Err("unexpected replacement-smoke argument".into());
    }

    let mut library = Library::open(config)?;
    let paper = library
        .list_papers(&ListFilter::default())?
        .into_iter()
        .find(|paper| paper.relative_path == relative_path)
        .ok_or("selected smoke-test paper was not scanned")?;
    library.store_bibtex(
        &paper.id,
        "@article{2008MNRAS.386..619C, title={Publisher replacement smoke test}, author={C, A}, year={2008}}",
    )?;
    let result = library.replace_pdf_from_file(&paper.id, publisher_pdf)?;
    println!("{}", result.active_path.display());
    for backup in result.backup_paths {
        println!("{}", backup.display());
    }
    Ok(())
}
