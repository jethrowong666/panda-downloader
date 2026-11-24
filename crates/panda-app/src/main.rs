// 发布版不弹出控制台窗口。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bridge;
mod category;
mod clipboard;
mod commands;
mod manager;
mod store;
mod sys;
mod video;
mod windows;

use std::sync::Arc;
use std::time::Duration;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager as _, RunEvent, WindowEvent};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use crate::manager::Manager;

fn main() {
    // 安装程序调用：只给浏览器注册扩展通信组件，然后退出（不启动界面）。
    if std::env::args().any(|a| a == "--register-host") {
        if let Some(d) = dirs::config_dir() {
            bridge::register_native_host(&d.join("com.panda.downloader"));
        }
        return;
    }
    let hotkey = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyD);
    tauri::Builder::default()
        // 第二次启动时：把参数里的链接交给已运行的实例，并显示主窗口。
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            let links: Vec<String> = args.iter().skip(1).filter(|a| !a.starts_with("--")).cloned().collect();
            if links.is_empty() {
                windows::show_main(app);
            } else if let Some(m) = app.try_state::<Arc<Manager>>() {
                m.capture(links);
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, sc, ev| {
                    if *sc == hotkey && ev.state() == ShortcutState::Pressed {
                        if let Some(m) = app.try_state::<Arc<Manager>>() {
                            if clipboard::capture_now(&m) == 0 {
                                windows::show_main(app);
                            }
                        }
                    }
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            commands::tasks,
            commands::group_tasks,
            commands::confirm_group,
            commands::cancel_group,
            commands::open_existing,
            commands::pause,
            commands::resume,
            commands::pause_all,
            commands::cancel,
            commands::undo_cancel,
            commands::remove,
            commands::open_file,
            commands::open_folder,
            commands::primary_action,
            commands::dismiss_done,
            commands::add_links,
            commands::get_settings,
            commands::set_settings,
            commands::history,
            commands::remove_history,
            commands::set_video_choice,
            commands::open_path,
            commands::pick_folder,
            commands::fit_window,
            commands::show_main,
            commands::free_space,
        ])
        .setup(move |app| {
            let data_dir = app.path().app_data_dir()?;
            let m = Manager::new(app.handle().clone(), data_dir);
            app.manage(m.clone());
            m.restore();

            // 心跳：刷新进度、控制窗口显示。
            let ticker = m.clone();
            tauri::async_runtime::spawn(async move {
                let mut n = 0u64;
                loop {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    ticker.tick(n);
                    n += 1;
                }
            });

            clipboard::start(m.clone());
            // 浏览器扩展的桥：本机接口 + 注册 Native Messaging 主机
            bridge::register_native_host(&app.path().app_data_dir()?);
            tauri::async_runtime::spawn(bridge::serve(m.clone(), app.handle().clone(), app.path().app_data_dir()?));
            let _ = app.global_shortcut().register(hotkey);

            let open = MenuItem::with_id(app, "open", "打开熊猫下载器", true, None::<&str>)?;
            let pause = MenuItem::with_id(app, "pause_all", "全部暂停", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &pause, &quit])?;
            let tray_mgr = m.clone();
            TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().cloned().expect("缺少图标"))
                .tooltip("熊猫下载器")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, ev| match ev.id().as_ref() {
                    "open" => windows::show_main(app),
                    "pause_all" => tray_mgr.pause_all(),
                    "quit" => {
                        // 退出前暂停所有任务，让进度落盘，下次启动接着下。
                        tray_mgr.pause_all();
                        std::thread::sleep(Duration::from_millis(600));
                        tray_mgr.persist();
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, ev| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                        windows::show_main(tray.app_handle());
                    }
                })
                .build(app)?;

            // 开机自启时带 --background，不弹主窗口。
            if !std::env::args().any(|a| a == "--background") {
                windows::show_main(app.handle());
            }
            Ok(())
        })
        .on_window_event(|win, ev| match ev {
            // 关闭主窗口 = 缩到托盘，下载继续。
            WindowEvent::CloseRequested { api, .. } if win.label() == "main" => {
                api.prevent_close();
                let _ = win.hide();
            }
            WindowEvent::Moved(pos) if win.label() == "float" => {
                if let Some(m) = win.app_handle().try_state::<Arc<Manager>>() {
                    m.save_float_pos(pos.x, pos.y);
                }
            }
            _ => {}
        })
        .build(tauri::generate_context!())
        .expect("启动失败")
        .run(|_app, ev| {
            // 所有窗口都关了也不退出（托盘里还在跑）。
            if let RunEvent::ExitRequested { api, code: None, .. } = ev {
                api.prevent_exit();
            }
        });
}
