use crate::bilibili::credential::Credential;
use crate::bilibili::{api_client, http_to_https};
use anyhow::{Context, Result};
use serde_json::{json, Map, Value};

/// 消息中心页面 Referer（模拟 web 端消息页）
const MESSAGE_REFERER: &str = "https://message.bilibili.com/";

/// 获取「@我的」通知列表（别人在评论/动态里 @ 了我）。
/// API: GET https://api.bilibili.com/x/msgfeed/at?build=0&mobi_app=web
/// - 分页为游标制：首次不传 id/time；翻页传上一页响应 cursor 里的 id 和 time。
/// - 时间字段为 `at_time`。
pub async fn get_at_feed(
    credential: &Credential,
    last_id: Option<i64>,
    last_time: Option<i64>,
) -> Result<Value> {
    // 翻页查询参数是 time，但条目上的时间戳字段叫 at_time
    get_feed(credential, "at", "time", "at_time", last_id, last_time).await
}

/// 获取「回复我的」通知列表（别人回复了我的评论/动态）。
/// API: GET https://api.bilibili.com/x/msgfeed/reply?build=0&mobi_app=web
/// - 分页为游标制：翻页参数 id + reply_time（取上一页 cursor.time）。
pub async fn get_reply_feed(
    credential: &Credential,
    last_id: Option<i64>,
    last_time: Option<i64>,
) -> Result<Value> {
    get_feed(
        credential,
        "reply",
        "reply_time",
        "reply_time",
        last_id,
        last_time,
    )
    .await
}

/// 获取「收到的赞」列表（别人点赞了我的评论/动态等）。
/// API: GET https://api.bilibili.com/x/msgfeed/like?build=0&mobi_app=web
/// - 响应为 latest（最近未看）+ total（历史游标）双列表，这里统一合并输出。
/// - 翻页参数 id + like_time（取上一页 cursor.time）。
pub async fn get_like_feed(
    credential: &Credential,
    last_id: Option<i64>,
    last_time: Option<i64>,
) -> Result<Value> {
    get_feed(
        credential,
        "like",
        "like_time",
        "like_time",
        last_id,
        last_time,
    )
    .await
}

/// 获取消息未读数（@/回复/点赞/投币/弹幕/收藏/系统通知等）。
/// API: GET https://api.bilibili.com/x/msgfeed/unread?build=0&mobi_app=web
/// 返回 data 原样（at/reply/like/recv_like/recv_reply 等计数字段）。
pub async fn get_unread(credential: &Credential) -> Result<Value> {
    let client = api_client();
    let resp_text = client
        .get("https://api.bilibili.com/x/msgfeed/unread")
        .header("Referer", MESSAGE_REFERER)
        .header("Cookie", credential.cookie_header())
        .query(&[("build", "0"), ("mobi_app", "web")])
        .send()
        .await?
        .text()
        .await?;

    let resp: Value = serde_json::from_str(&resp_text).context("消息未读数响应解析失败")?;
    ensure_ok(&resp)?;
    Ok(resp["data"].clone())
}

/// 三种通知列表共用请求逻辑（路径 feed = at/reply/like）。
/// 翻页时间参数名不同：at 传 time，reply 传 reply_time，like 传 like_time；
/// 条目上的时间戳字段也不完全一致（at 条目是 at_time），故分开传。
async fn get_feed(
    credential: &Credential,
    feed: &str,
    time_param: &str,
    item_ts_field: &str,
    last_id: Option<i64>,
    last_time: Option<i64>,
) -> Result<Value> {
    let client = api_client();
    let url = format!("https://api.bilibili.com/x/msgfeed/{feed}");
    let mut req = client
        .get(&url)
        .header("Referer", MESSAGE_REFERER)
        .header("Cookie", credential.cookie_header())
        .query(&[("build", "0"), ("mobi_app", "web"), ("platform", "web")]);
    if let Some(id) = last_id {
        req = req.query(&[("id", id.to_string())]);
    }
    if let Some(time) = last_time {
        req = req.query(&[(time_param, time.to_string())]);
    }

    let resp_text = req.send().await?.text().await?;
    log::debug!(
        "[msgfeed] GET {} 响应前 300 字: {}",
        url,
        resp_text.chars().take(300).collect::<String>()
    );

    let resp: Value = serde_json::from_str(&resp_text).context("消息通知响应解析失败")?;
    ensure_ok(&resp)?;
    Ok(trim_feed(&resp["data"], item_ts_field))
}

fn ensure_ok(resp: &Value) -> Result<()> {
    let code = resp["code"].as_i64().unwrap_or(-1);
    if code != 0 {
        anyhow::bail!(
            "获取消息通知失败: code={}（{}）",
            code,
            resp["message"].as_str().unwrap_or("未知错误")
        );
    }
    Ok(())
}

/// 通知列表 → 精简 JSON：{items, cursor}（like 为 {latest, items, cursor}）。
fn trim_feed(data: &Value, ts_field: &str) -> Value {
    // like 接口是 {latest:{items,last_view_at}, total:{cursor,items}} 双列表
    if data["total"].is_object() {
        return json!({
            "latest": trim_items(&data["latest"]["items"], ts_field),
            "items": trim_items(&data["total"]["items"], ts_field),
            "cursor": trim_cursor(&data["total"]["cursor"]),
        });
    }
    json!({
        "items": trim_items(&data["items"], ts_field),
        "cursor": trim_cursor(&data["cursor"]),
    })
}

fn trim_items(list: &Value, ts_field: &str) -> Value {
    let items = list.as_array().cloned().unwrap_or_default();
    Value::Array(
        items
            .iter()
            .map(|item| trim_notice(item, ts_field))
            .collect(),
    )
}

fn trim_cursor(cursor: &Value) -> Value {
    json!({
        "is_end": cursor["is_end"].as_bool().unwrap_or(true),
        "id": cursor["id"].as_i64().unwrap_or(0),
        "time": cursor["time"].as_i64().unwrap_or(0),
    })
}

/// 单条通知 → 精简 JSON：id、时间、发送者、正文摘要、跳转链接。
/// at/reply/like 三种通知的 item 字段名有差异，按存在与否选择性拷贝。
fn trim_notice(item: &Value, ts_field: &str) -> Value {
    let mut out = Map::new();
    out.insert("id".into(), json!(item["id"].as_i64().unwrap_or(0)));
    out.insert(ts_field.into(), json!(item[ts_field].as_i64().unwrap_or(0)));
    out.insert("user".into(), trim_user(&item["user"]));

    let detail = &item["item"];
    let mut info = Map::new();
    for key in [
        "subject_id",
        "subject_title",
        "title",
        "business",
        "business_id",
        "root_id",
        "parent_id",
        "source_id",
        "target_id",
        "target_reply_id",
        "message",
        "source_content",
        "target_reply_content",
        "root_reply_content",
        "uri",
        "time",
    ] {
        let v = &detail[key];
        if v.is_null() {
            continue;
        }
        if let Some(s) = v.as_str() {
            if s.is_empty() {
                continue;
            }
        }
        info.insert(key.to_string(), v.clone());
    }
    // at_details：被@的用户列表，精简为 (mid, nickname)
    if let Some(ats) = detail["at_details"].as_array() {
        if !ats.is_empty() {
            info.insert(
                "at_details".into(),
                json!(ats
                    .iter()
                    .map(|u| {
                        json!({
                            "mid": u["mid"].as_i64().unwrap_or(0),
                            "nickname": u["nickname"].as_str().unwrap_or(""),
                        })
                    })
                    .collect::<Vec<_>>()),
            );
        }
    }
    out.insert("item".into(), Value::Object(info));
    Value::Object(out)
}

fn trim_user(user: &Value) -> Value {
    json!({
        "mid": user["mid"].as_i64().unwrap_or(0),
        "nickname": user["nickname"].as_str().unwrap_or(""),
        "avatar": http_to_https(user["avatar"].as_str().unwrap_or("")),
        "follow": user["follow"].as_bool().unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn trim_notice_extracts_at_item() {
        let notice = trim_notice(
            &json!({
                "id": 823260581625886i64,
                "user": {
                    "mid": 3546910497441845i64,
                    "fans": 0,
                    "nickname": "张三",
                    "avatar": "http://i2.hdslb.com/bfs/face/x.jpg",
                    "mid_link": "",
                    "follow": false
                },
                "item": {
                    "subject_id": 1234,
                    "subject_title": "视频标题",
                    "root_id": 0,
                    "parent_id": 0,
                    "target_reply_id": 265141324256i64,
                    "business_id": 1,
                    "message": "@我 看看这个",
                    "image_urls": null,
                    "time": 1749474709,
                    "uri": "https://www.bilibili.com/video/av1234"
                },
                "at_time": 1749474710
            }),
            "at_time",
        );
        assert_eq!(notice["id"], 823260581625886i64);
        assert_eq!(notice["at_time"], 1749474710);
        assert_eq!(notice["user"]["nickname"], "张三");
        assert_eq!(
            notice["user"]["avatar"],
            "https://i2.hdslb.com/bfs/face/x.jpg"
        );
        assert_eq!(notice["item"]["subject_title"], "视频标题");
        assert_eq!(notice["item"]["message"], "@我 看看这个");
        // root_id=0 属于存在但无意义，仍保留（数字 0 不跳过，只跳过空串/null）
        assert_eq!(notice["item"]["root_id"], 0);
    }

    #[test]
    fn trim_notice_skips_empty_strings_and_nulls() {
        let notice = trim_notice(
            &json!({
                "id": 1,
                "user": {"mid": 2, "nickname": "李四"},
                "item": {
                    "subject_id": 100,
                    "title": "被点赞的内容",
                    "uri": "",
                    "danmu": null,
                    "message": ""
                },
                "like_time": 1700000000
            }),
            "like_time",
        );
        assert_eq!(notice["like_time"], 1700000000);
        assert_eq!(notice["item"]["title"], "被点赞的内容");
        assert!(notice["item"].get("uri").is_none());
        assert!(notice["item"].get("message").is_none());
        assert!(notice["item"].get("danmu").is_none());
    }

    #[test]
    fn trim_notice_keeps_at_details() {
        let notice = trim_notice(
            &json!({
                "id": 3,
                "user": {"mid": 4, "nickname": "王五"},
                "item": {
                    "subject_id": 100,
                    "source_content": "回复内容",
                    "at_details": [
                        {"mid": 9, "nickname": "被@的人", "fans": 0, "follow": false}
                    ]
                },
                "reply_time": 1700000001
            }),
            "reply_time",
        );
        assert_eq!(notice["item"]["at_details"][0]["mid"], 9);
        assert_eq!(notice["item"]["at_details"][0]["nickname"], "被@的人");
        assert_eq!(notice["item"]["source_content"], "回复内容");
    }

    #[test]
    fn trim_feed_normalizes_cursor_envelope() {
        let feed = trim_feed(
            &json!({
                "cursor": {"is_end": true, "id": 823260581625886i64, "time": 1749474709},
                "items": [],
                "last_view_at": 1749474724
            }),
            "reply_time",
        );
        assert_eq!(feed["cursor"]["is_end"], true);
        assert_eq!(feed["cursor"]["id"], 823260581625886i64);
        assert!(feed["items"].is_array());
        assert!(feed.get("latest").is_none());
    }

    #[test]
    fn trim_feed_normalizes_like_envelope() {
        let feed = trim_feed(
            &json!({
                "latest": {"items": [{"id": 1, "like_time": 1, "user": {"mid": 2, "nickname": "a"}, "item": {"subject_id": 5}}], "last_view_at": 0},
                "total": {
                    "cursor": {"is_end": false, "id": 9, "time": 123},
                    "items": [{"id": 7, "like_time": 2, "user": {"mid": 3, "nickname": "b"}, "item": {"subject_id": 6}}]
                }
            }),
            "like_time",
        );
        assert_eq!(feed["latest"][0]["id"], 1);
        assert_eq!(feed["latest"][0]["item"]["subject_id"], 5);
        assert_eq!(feed["items"][0]["id"], 7);
        assert_eq!(feed["cursor"]["is_end"], false);
        assert_eq!(feed["cursor"]["time"], 123);
    }
}
