//! Relationships between typed pipe fields that serde cannot express.

use crate::pipe_command::{EmulateArgs, PipeCommand, WaitArgs};

pub fn validate(command: &PipeCommand) -> Result<(), crate::BoxError> {
    match command {
        PipeCommand::Click(args) | PipeCommand::Dblclick(args) => {
            exactly_one(
                command.name(),
                &[
                    args.uid.is_some(),
                    args.selector.is_some(),
                    args.xy.is_some(),
                ],
                "\"uid\", \"selector\", or \"xy\"",
            )?;
            on_intercept(args.on_intercept.as_deref())?;
        }
        PipeCommand::Fill(args) => exactly_one(
            command.name(),
            &[args.uid.is_some(), args.selector.is_some()],
            "\"uid\" or \"selector\"",
        )?,
        PipeCommand::Select(args) => exactly_one(
            command.name(),
            &[args.uid.is_some(), args.selector.is_some()],
            "\"uid\" or \"selector\"",
        )?,
        PipeCommand::Check(args) => {
            exactly_one(
                command.name(),
                &[args.uid.is_some(), args.selector.is_some()],
                "\"uid\" or \"selector\"",
            )?;
            on_intercept(args.on_intercept.as_deref())?;
        }
        PipeCommand::Uncheck(args) => {
            exactly_one(
                command.name(),
                &[args.uid.is_some(), args.selector.is_some()],
                "\"uid\" or \"selector\"",
            )?;
            on_intercept(args.on_intercept.as_deref())?;
        }
        PipeCommand::Upload(args) => exactly_one(
            command.name(),
            &[args.uid.is_some(), args.selector.is_some()],
            "\"uid\" or \"selector\"",
        )?,
        PipeCommand::Text(args) => at_most_one(
            command.name(),
            &[args.uid.is_some(), args.selector.is_some()],
            "\"uid\" or \"selector\"",
        )?,
        PipeCommand::Screenshot(args) => at_most_one(
            command.name(),
            &[args.uid.is_some(), args.selector.is_some()],
            "\"uid\" or \"selector\"",
        )?,
        PipeCommand::Download(args) => {
            exactly_one(
                command.name(),
                &[
                    args.url.is_some(),
                    args.uid.is_some(),
                    args.selector.is_some(),
                ],
                "\"url\", \"uid\", or \"selector\"",
            )?;
            on_intercept(args.on_intercept.as_deref())?;
        }
        PipeCommand::FillAndSubmit(args) => on_intercept(args.on_intercept.as_deref())?,
        PipeCommand::Wait(args) => wait_shape(args)?,
        PipeCommand::Emulate(args) => emulate_shape(args)?,
        _ => {}
    }
    Ok(())
}

fn exactly_one(name: &str, present: &[bool], choices: &str) -> Result<(), crate::BoxError> {
    if present.iter().filter(|&&set| set).count() == 1 {
        Ok(())
    } else {
        Err(format!("{name}: provide exactly one of {choices}").into())
    }
}

fn at_most_one(name: &str, present: &[bool], choices: &str) -> Result<(), crate::BoxError> {
    if present.iter().filter(|&&set| set).count() <= 1 {
        Ok(())
    } else {
        Err(format!("{name}: provide at most one of {choices}").into())
    }
}

fn on_intercept(value: Option<&str>) -> Result<(), crate::BoxError> {
    value.map_or(Ok(()), |value| {
        crate::hit_test::OnIntercept::parse(value)
            .map(|_| ())
            .map_err(Into::into)
    })
}

fn wait_shape(args: &WaitArgs) -> Result<(), crate::BoxError> {
    let explicit = args.what.is_some() || args.pattern.is_some();
    let shortcuts = [
        args.text.is_some(),
        args.url.is_some(),
        args.selector.is_some(),
    ]
    .into_iter()
    .filter(|set| *set)
    .count();
    if args.pattern.is_some() && args.what.is_none() {
        return Err("wait: \"pattern\" requires \"what\"".into());
    }
    if shortcuts > 1 || (explicit && shortcuts != 0) {
        return Err("wait: provide either \"what\"/\"pattern\" or one shorthand target".into());
    }
    Ok(())
}

fn emulate_shape(args: &EmulateArgs) -> Result<(), crate::BoxError> {
    let device_fields = [
        args.label.is_some(),
        args.width.is_some(),
        args.height.is_some(),
        args.dpr.is_some(),
        args.mobile.is_some(),
        args.touch.is_some(),
        args.orientation.is_some(),
    ];
    if matches!(args.action.as_deref(), Some("status" | "reset"))
        && device_fields.into_iter().any(|set| set)
    {
        return Err(format!(
            "emulate {}: device fields are only valid for action \"device\"",
            args.action.as_deref().unwrap_or_default()
        )
        .into());
    }
    Ok(())
}
