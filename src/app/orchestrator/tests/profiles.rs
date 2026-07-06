//! Тесты оркестратора — профили: создание/удаление/каскад. Часть модуля [`super`]
//! (фикстуры в mod.rs). См. docs/refactoring-god-objects.md, этап 3.

use super::*;

#[tokio::test]
async fn create_profile_appears_in_profile_list() {
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    // Бутстрап создаёт один профиль по умолчанию.
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ProfileList(p) if p.len() == 1),
    )
    .await
    .unwrap();

    cmd_tx
        .send(AppCommand::CreateProfile {
            name: "  Второй  ".into(),
            system_message: "sys".into(),
        })
        .unwrap();
    let list = wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ProfileList(p) if p.len() == 2),
    )
    .await
    .unwrap();
    if let AppEvent::ProfileList(profiles) = list {
        assert!(profiles.iter().any(|p| p.name == "Второй")); // имя нормализовано
    }

    drop(cmd_tx);
    handle.await.unwrap();
}

#[tokio::test]
async fn delete_profile_cascades_to_its_chats() {
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ChatActivated { .. }))
        .await
        .unwrap();

    // Создаём второй профиль и узнаём его id.
    cmd_tx
        .send(AppCommand::CreateProfile {
            name: "Второй".into(),
            system_message: "sys".into(),
        })
        .unwrap();
    let list = wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ProfileList(p) if p.len() == 2),
    )
    .await
    .unwrap();
    let second_id = match list {
        AppEvent::ProfileList(profiles) => profiles.iter().find(|p| p.name == "Второй").unwrap().id,
        _ => unreachable!(),
    };

    // Создаём чат из второго профиля → всего два чата.
    cmd_tx
        .send(AppCommand::NewChat {
            profile_id: Some(second_id),
        })
        .unwrap();
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ChatList(c) if c.len() == 2),
    )
    .await
    .unwrap();

    // Удаляем второй профиль: его чат каскадно скрывается → остаётся один.
    cmd_tx.send(AppCommand::DeleteProfile(second_id)).unwrap();
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ProfileList(p) if p.len() == 1),
    )
    .await
    .unwrap();
    wait_for(
        &mut evt_rx,
        |e| matches!(e, AppEvent::ChatList(c) if c.len() == 1),
    )
    .await
    .unwrap();

    drop(cmd_tx);
    handle.await.unwrap();
}

#[tokio::test]
async fn cannot_delete_last_profile() {
    let (_d, cmd_tx, mut evt_rx, handle) = spawn_orch(None);
    let list = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::ProfileList(_)))
        .await
        .unwrap();
    let only_id = match list {
        AppEvent::ProfileList(p) => p[0].id,
        _ => unreachable!(),
    };

    cmd_tx.send(AppCommand::DeleteProfile(only_id)).unwrap();
    let err = wait_for(&mut evt_rx, |e| matches!(e, AppEvent::Error(_)))
        .await
        .unwrap();
    assert!(matches!(err, AppEvent::Error(_)));

    drop(cmd_tx);
    handle.await.unwrap();
}
