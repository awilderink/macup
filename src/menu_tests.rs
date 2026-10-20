use super::*;

fn ready(backup_on_drive: bool, last_backup: Option<Duration>, set_up: bool) -> Situation {
    Situation {
        drive: Drive::Ready,
        backup_on_drive,
        last_backup,
        set_up,
    }
}

fn action(s: &Situation) -> Option<Action> {
    recommend(s).map(|(a, _)| a)
}

#[test]
fn a_fresh_mac_with_a_backup_on_the_drive_is_offered_setup() {
    assert_eq!(action(&ready(true, None, false)), Some(Action::Setup));
}

// This is the first-backup guard's case: recommending "Back up" here would lead
// straight into a warning that the backup is about to be overwritten.
#[test]
fn a_set_up_mac_that_never_backed_up_is_offered_restore_not_backup() {
    assert_eq!(action(&ready(true, None, true)), Some(Action::Restore));
}

#[test]
fn backup_is_recommended_once_a_day_has_passed() {
    let hour = Duration::from_secs(3600);
    assert_eq!(action(&ready(true, Some(hour), true)), None);
    let (a, why) = recommend(&ready(true, Some(DAY * 6), true)).unwrap();
    assert_eq!(a, Action::Backup);
    assert_eq!(why, "the last backup was 6 days ago");
    assert_eq!(
        recommend(&ready(true, Some(DAY), true)).unwrap().1,
        "the last backup was yesterday"
    );
}

#[test]
fn an_empty_drive_on_a_new_mac_is_offered_a_first_backup() {
    assert_eq!(action(&ready(false, None, false)), Some(Action::Backup));
}

#[test]
fn a_connected_drive_without_macup_toml_is_offered_for_backups() {
    let s = Situation {
        drive: Drive::Unprepared(vec!["/Volumes/Spare".into()]),
        backup_on_drive: false,
        last_backup: None,
        set_up: true,
    };
    let (a, why) = recommend(&s).unwrap();
    assert_eq!(a, Action::UseDrive);
    assert!(why.starts_with("Spare is connected"), "{why}");
}

#[test]
fn nothing_is_pushed_without_a_drive() {
    let s = Situation {
        drive: Drive::Missing,
        backup_on_drive: false,
        last_backup: None,
        set_up: false,
    };
    assert_eq!(action(&s), None);
}

#[test]
fn the_real_situation_is_cheap_to_read_and_consistent() {
    let Ok(e) = Env::load() else { return };
    let s = situation(&e);
    if let Some((a, _)) = recommend(&s) {
        assert!(
            options(&s).iter().any(|(_, o)| *o == a),
            "recommended {a:?} is not in the menu"
        );
    }
}
