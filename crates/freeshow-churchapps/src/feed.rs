// SPDX-License-Identifier: GPL-3.0-only

use super::get_json;
use serde_json::{Value, json};
use std::collections::HashMap;

fn request(base: &str, path: &str) -> Result<Value, String> {
    // Lesson feeds/add-ons are public; never attach the service account token.
    serde_json::from_str(&get_json(&format!("{base}/{path}"), None)?)
        .map_err(|error| error.to_string())
}

fn path_id(id: &str) -> Result<&str, String> {
    if id.is_empty()
        || id == "."
        || id == ".."
        || id.contains(['/', '\\', '?', '#', '%'])
        || id.chars().any(char::is_control)
    {
        return Err("invalid Lessons.church resource ID".into());
    }
    Ok(id)
}

fn playable(action: &Value) -> bool {
    action["actionType"].as_str().is_some_and(|kind| {
        kind.eq_ignore_ascii_case("play") || kind.eq_ignore_ascii_case("add-on")
    })
}

pub(super) fn hydrate_plan(
    base: &str,
    plan: &mut Value,
    cache: &mut HashMap<String, Value>,
) -> Result<(), String> {
    let mut feed = Value::Null;
    if plan["contentType"] == "venue" {
        let venue = plan["contentId"]
            .as_str()
            .ok_or("venue plan has no content ID")?;
        let key = format!("venue:{venue}");
        if !cache.contains_key(&key) {
            cache.insert(
                key.clone(),
                request(base, &format!("venues/public/feed/{}", path_id(venue)?))?,
            );
        }
        feed = cache[&key].clone();
        if !feed["sections"].is_array() {
            return Err("Lessons.church venue feed has no sections".into());
        }
    }
    if plan["items"].as_array().is_none_or(Vec::is_empty) && feed["sections"].is_array() {
        // A venue service with no custom order follows the feed's sections.
        let mut items = Vec::new();
        for (section_index, section) in feed["sections"].as_array().unwrap().iter().enumerate() {
            let mut children = Vec::new();
            for (action_index, action) in section["actions"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
            {
                if !playable(action)
                    || !action["files"]
                        .as_array()
                        .is_some_and(|files| !files.is_empty())
                {
                    continue;
                }
                let id = action["id"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| {
                        format!(
                            "{}_s{section_index}_a{action_index}",
                            plan["contentId"].as_str().unwrap()
                        )
                    });
                let seconds: f64 = action["files"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|file| file["seconds"].as_f64())
                    .sum();
                children.push(json!({"id":id,"itemType":"lessonAction","label":action["content"].as_str().filter(|name| !name.is_empty()).or(section["name"].as_str()).unwrap_or(""),"files":action["files"],"seconds":seconds}));
            }
            items.push(json!({"label":section["name"],"children":children}));
        }
        plan["items"] = json!(items);
    }

    for item in plan["items"].as_array_mut().into_iter().flatten() {
        for child in item["children"].as_array_mut().into_iter().flatten() {
            let kind = child["itemType"].as_str().unwrap_or("").to_string();
            if !["lessonSection", "lessonAction", "lessonAddOn"].contains(&kind.as_str()) {
                continue;
            }
            if child["files"]
                .as_array()
                .is_some_and(|files| !files.is_empty())
            {
                continue;
            }
            let related = child["relatedId"]
                .as_str()
                .ok_or("lesson item has no related ID")?;
            let mut files = Vec::new();
            for section in feed["sections"].as_array().into_iter().flatten() {
                for action in section["actions"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|action| playable(action))
                {
                    if (kind == "lessonSection" && section["id"] == related)
                        || (kind != "lessonSection" && action["id"] == related)
                    {
                        files.extend(action["files"].as_array().into_iter().flatten().cloned());
                    }
                }
            }
            if files.is_empty() && kind == "lessonAddOn" {
                if let Some(add_on) = feed["files"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|entry| entry["id"] == related)
                {
                    files = add_on["files"].as_array().cloned().unwrap_or_default();
                }
            }
            if files.is_empty() && kind == "lessonAddOn" {
                let key = format!("addon:{related}");
                if !cache.contains_key(&key) {
                    cache.insert(
                        key.clone(),
                        request(base, &format!("addOns/public/{}", path_id(related)?))?,
                    );
                }
                let data = &cache[&key];
                if let Some(video_id) = data["video"]["id"].as_str() {
                    files.push(json!({"name":data["name"],"url":format!("{base}/externalVideos/download/{}",path_id(video_id)?),"fileType":"video/mp4","loopVideo":data["video"]["loopVideo"],"seconds":data["video"]["seconds"]}));
                } else if let Some(url) = data["file"]["contentPath"].as_str() {
                    files.push(
                        json!({"name":data["name"],"url":url,"fileType":data["file"]["fileType"]}),
                    );
                }
            }
            if files.is_empty() {
                return Err(format!(
                    "lesson item has no media in its venue feed ({kind})"
                ));
            }
            child["files"] = json!(files);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lessons::tests::{response, server_with};
    use crate::{fetch_plans_from, plans_from_document, write_plans};
    use freeshow_data::{Library, ProjectsFile, SlideBackdrop};
    use std::fs;

    #[test]
    fn live_service_resolves_sections_actions_and_addons_then_downloads_local_media() {
        let root =
            std::env::temp_dir().join(format!("freeshow-plan-lessons-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let png = include_bytes!("../../freeshow-data/tests/fixtures/lesson-cover.png");
        let clip = include_bytes!("../../freeshow-data/tests/fixtures/lesson-clip.mp4");
        let (base, server) = server_with(|base| {
            let plans = json!([{"id":"p1","churchId":"c1","name":"Sunday","contentType":"venue","contentId":"v1"}]);
            let items = json!([{"label":"Kids","description":"Leader notes","children":[
                {"id":"section","itemType":"lessonSection","relatedId":"s1","label":"Opening","seconds":12.5},
                {"id":"action","itemType":"lessonAction","relatedId":"a2","label":"Story","seconds":8},
                {"id":"extra","itemType":"lessonAddOn","relatedId":"external","label":"Extra","seconds":10},
                {"id":"cached","itemType":"lessonAddOn","relatedId":"cached","label":"Cached"},
                {"id":"alias","itemType":"lessonAddOn","relatedId":"a2","label":"Action alias"},
                {"id":"welcome","itemType":"item","label":"Welcome","description":"Good morning"}
            ]}]);
            let feed = json!({"sections":[{"id":"s1","name":"Opening","actions":[
                {"id":"a1","actionType":"play","files":[{"name":"Title","url":format!("{base}/cover.png")}]},
                {"id":"a2","actionType":"add-on","files":[{"name":"Story","url":format!("{base}/story.mp4"),"loopVideo":true}]},
                {"id":"ignored","actionType":"say","files":[{"url":"file:///not-downloaded.png"}]}
            ]}],"files":[{"id":"cached","files":[{"name":"Cover","url":format!("{base}/cover.png")}]}]});
            let addon = json!({"name":"Extra","video":{"id":"external-video","loopVideo":false}});
            vec![
                response(plans.to_string().as_bytes()),
                response(items.to_string().as_bytes()),
                response(feed.to_string().as_bytes()),
                response(addon.to_string().as_bytes()),
                response(png),
                response(clip),
                response(clip),
            ]
        });
        let document = fetch_plans_from(&base, &base, "test-account-token").unwrap();
        let plans = plans_from_document(&document).unwrap();
        assert_eq!(write_plans(&root, &plans).unwrap(), 6);
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 7);
        assert!(requests[0].starts_with("GET /doing/plans/presenter "));
        assert!(requests[1].starts_with("GET /doing/planItems/presenter/c1/p1 "));
        assert!(requests[..2].iter().all(|r| {
            r.to_ascii_lowercase()
                .contains("authorization: bearer test-account-token")
        }));
        assert!(
            requests[2..]
                .iter()
                .all(|r| !r.to_ascii_lowercase().contains("authorization:"))
        );
        assert!(requests[2].starts_with("GET /venues/public/feed/v1 "));
        assert!(requests[3].starts_with("GET /addOns/public/external "));
        assert!(requests[6].starts_with("GET /externalVideos/download/external-video "));
        let library = Library::open(&root);
        let shows = library.list_shows().unwrap();
        let section = shows
            .iter()
            .find(|show| show.id == "chumslesson_section")
            .unwrap();
        assert_eq!(section.ordered_slides().len(), 2);
        assert_eq!(
            section.show["media"]["m1"]["loop"], false,
            "service Title files do not force a loop"
        );
        assert_eq!(section.show["media"]["m2"]["loop"], true);
        assert_eq!(section.show["media"]["m2"]["muted"], false);
        assert!(!section.nexts_after_media_at(0));
        let SlideBackdrop::Picture { path, .. } = section.backdrop_at(1) else {
            panic!("missing clip")
        };
        assert_eq!(fs::read(path).unwrap(), clip);
        let action = shows
            .iter()
            .find(|show| show.id == "chumsaction_action")
            .unwrap();
        assert_eq!(action.backdrop_at(0), section.backdrop_at(1));
        let projects = ProjectsFile::load(&library.projects_path()).unwrap();
        let rows = &projects.projects()[0].shows;
        assert_eq!(rows.len(), 7);
        assert_eq!(rows[1].id, "chumslesson_section");
        assert_eq!(rows[6].id, "chumsshow_welcome");
        let saved: Value = serde_json::from_str(&projects.to_json().unwrap()).unwrap();
        assert_eq!(saved["projects"]["p1"]["shows"][1]["scheduleLength"], 12.5);
        // The fully assembled service document reimports without the API or CDN.
        assert_eq!(write_plans(&root, &plans).unwrap(), 6);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn metadata_or_media_failure_keeps_the_previous_service_and_show_files() {
        let root = std::env::temp_dir().join(format!(
            "freeshow-plan-lesson-failure-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let original = json!({"plans":[{"id":"p1","name":"Sunday","items":[{"label":"Welcome","children":[
            {"itemType":"item","id":"welcome","label":"Welcome","description":"Original words"}
        ]}]}]});
        write_plans(&root, &plans_from_document(&original.to_string()).unwrap()).unwrap();
        let library = Library::open(&root);
        let project = fs::read(library.projects_path()).unwrap();
        let original_show = fs::read(library.shows_dir().join("chumsshow_welcome.show")).unwrap();
        let (base, server) = server_with(|_| {
            vec![
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_vec(),
            ]
        });
        let mut changed = original.clone();
        let children = changed["plans"][0]["items"][0]["children"]
            .as_array_mut()
            .unwrap();
        children[0]["description"] = json!("Must not replace the original");
        children.push(json!({"id":"missing","itemType":"lessonAction","label":"Missing clip","files":[{"url":format!("{base}/missing.mp4")}]}));
        let plans = plans_from_document(&changed.to_string()).unwrap();
        assert!(write_plans(&root, &plans).unwrap_err().contains("404"));
        server.join().unwrap();
        assert_eq!(fs::read(library.projects_path()).unwrap(), project);
        assert_eq!(
            fs::read(library.shows_dir().join("chumsshow_welcome.show")).unwrap(),
            original_show
        );
        assert_eq!(library.list_shows().unwrap().len(), 1);
        changed["plans"][0]["items"][0]["children"][1]
            .as_object_mut()
            .unwrap()
            .remove("files");
        assert!(
            plans_from_document(&changed.to_string())
                .unwrap_err()
                .contains("resolved media")
        );

        let (base, server) = server_with(|_| {
            vec![
            response(br#"[{"id":"p1","churchId":"c1","contentType":"venue","contentId":"v1"}]"#),
            response(br#"[{"children":[{"id":"missing","itemType":"lessonSection","relatedId":"section"}]}]"#),
            b"HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
        ]
        });
        assert!(
            fetch_plans_from(&base, &base, "token")
                .unwrap_err()
                .contains("503")
        );
        server.join().unwrap();
        assert_eq!(fs::read(library.projects_path()).unwrap(), project);
        assert_eq!(
            fs::read(library.shows_dir().join("chumsshow_welcome.show")).unwrap(),
            original_show
        );
        assert!(
            crate::lessons::plan_media_show(
                &library,
                "invalid",
                "Invalid",
                "",
                &[json!("not a media object")]
            )
            .is_err()
        );
        assert!(
            crate::lessons::plan_media_show(
                &library,
                "stream",
                "Stream",
                "",
                &[json!({"streamUrl":"https://vimeo.com/123","fileType":"video"})]
            )
            .unwrap_err()
            .contains("downloadable")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn venue_order_without_custom_items_uses_play_and_addon_actions_in_order() {
        let mut cache = HashMap::from([(
            "venue:v1".to_string(),
            json!({"sections":[
                {"id":"opening","name":"Opening","actions":[
                    {"id":"a1","actionType":"Play","content":"Welcome","files":[{"url":"https://cdn.invalid/welcome.png","seconds":2.5}]},
                    {"id":"skip","actionType":"say","content":"Leader notes"}
                ]},
                {"name":"Story","actions":[{"actionType":"add-on","files":[{"url":"https://cdn.invalid/story.mp4","seconds":30}]}]}
            ]}),
        )]);
        let mut plan =
            json!({"id":"p1","name":"Sunday","contentType":"venue","contentId":"v1","items":[]});
        hydrate_plan("http://127.0.0.1:1", &mut plan, &mut cache).unwrap();
        assert_eq!(plan["items"][0]["children"][0]["id"], "a1");
        assert_eq!(plan["items"][0]["children"].as_array().unwrap().len(), 1);
        assert_eq!(plan["items"][1]["children"][0]["id"], "v1_s1_a0");
        assert_eq!(plan["items"][1]["children"][0]["seconds"], 30.0);
        assert_eq!(
            plans_from_document(&json!({"plans":[plan]}).to_string())
                .unwrap()
                .len(),
            1
        );
        let mut standalone = json!({"id":"p2","items":[{"children":[{"id":"doc","itemType":"lessonAddOn","relatedId":"document"}]}]});
        cache.insert("addon:document".into(), json!({"name":"Sheet","file":{"contentPath":"https://cdn.invalid/sheet.png","fileType":"image/png"}}));
        hydrate_plan("http://127.0.0.1:1", &mut standalone, &mut cache).unwrap();
        assert_eq!(
            standalone["items"][0]["children"][0]["files"][0]["url"],
            "https://cdn.invalid/sheet.png"
        );
        assert!(path_id("../escape").is_err());
    }
}
