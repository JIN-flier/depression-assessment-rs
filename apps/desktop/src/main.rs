fn main() -> Result<(), Box<dyn std::error::Error>> {
    let desktop = depression_desktop::DesktopApp::new()?;
    desktop.run()?;
    Ok(())
}
