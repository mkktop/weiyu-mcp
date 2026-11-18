use crate::{
    error::{QueryError, Result},
    input,
    settings::RuntimeSettings,
};
use rmcp::model::{Tool, ToolAnnotations};
use schemars::JsonSchema;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    sync::{Arc, OnceLock, RwLock},
    time::Duration,
};
use tokio::{
    sync::{Mutex, Semaphore},
    time::Instant,
};
use weiyu_core::bilibili::{self as bili, credential::Credential};

macro_rules! params {
    ($name:ident { $($(#[$meta:meta])* $field:ident: $ty:ty),* $(,)? }) => {
        #[derive(Deserialize, JsonSchema)]
        #[serde(deny_unknown_fields)]
        struct $name { $($(#[$meta])* $field: $ty),* }
    };
}
params!(Empty {});
params!(Link {
    /// 完整B站链接、BV号或av号
    url: String
});
params!(Bvid {
    /// 完整12位BV号
    bvid: String
});
params!(Search { keyword: String, search_type: Option<String>, page: Option<u32>, order: Option<String>, duration: Option<String>, tids: Option<String> });
params!(Limit { /// 条数上限（默认20，最多50）
    limit: Option<u32> });
params!(Suggest { term: String });
params!(Comments { bvid: String, page: Option<u32>, /// hot（默认）或time
    sort: Option<String> });
params!(Replies { bvid: String, root_rpid: i64, page: Option<u32> });
params!(Part {
    bvid: String,
    cid: i64
});
params!(AiSummary {
    bvid: String,
    cid: i64,
    up_mid: i64
});
params!(Danmaku { bvid: String, cid: i64, /// 时长，秒；最多21600秒
    duration_seconds: u32, limit: Option<u32> });
params!(Subtitle { /// 来自get_video_subtitles的字幕URL
    subtitle_url: String, /// 返回条数，默认1000，最多5000
    limit: Option<u32>, /// 从这个时间开始，单位秒
    from_seconds: Option<f64> });
params!(Article { cvid: Option<u64>, /// opus ID必须用字符串，保留19位精度
    opus_id: Option<String> });
params!(Mid { mid: i64 });
params!(UpperVideos { mid: i64, page: Option<u32>, keyword: Option<String> });
params!(Page { page: Option<u32> });
params!(Followers { mid: i64, page: Option<u32> });
params!(Ranking { rid: Option<u32> });
params!(Region { rid: Option<u32>, page_size: Option<u32> });
params!(PgcRank { season_type: Option<u8> });
params!(Bangumi { follow_type: Option<u8>, page: Option<u32> });
params!(Weekly { number: Option<i64> });
params!(Recommend { fresh_idx: Option<u32> });
params!(Dynamic { offset: Option<String> });
params!(HistoryCursor { view_at: i64, business: Option<String>, max: Option<String> });
params!(History { /// get_watch_history返回的next游标，需整体原样传回
    cursor: Option<HistoryCursor> });
params!(Favorite { media_id: String, page: Option<u32> });
params!(Collection { mid: i64, sid: String, /// season或series
    collection_type: String, page: Option<u32> });

// ==================== 互动写入参数（默认不暴露） ====================
params!(Like { bvid: String, /// true=点赞（默认），false=取消点赞
    like: Option<bool> });
params!(Coin { bvid: String, /// 投币数量：1（默认）或2
    multiply: Option<u8>, /// 是否同时点赞，默认false
    like_too: Option<bool> });
params!(FavoriteVideo {
    bvid: String,
    /// 收藏夹ID，来自get_favorite_folders
    folder_id: i64
});
params!(WatchLaterAdd { bvid: String });
params!(WatchLaterRemove { bvid: String });

// ==================== 评论写入参数（独立开关，默认不暴露） ====================
params!(Mention {
    /// B站用户 mid
    mid: i64,
    /// 正文中 @ 的昵称，必须与B站昵称一致
    name: String
});
params!(PostComment {
    bvid: String,
    /// 评论正文，最多1000字符
    message: String,
    /// 回复目标评论的 rpid；回复一级评论时只需传它，root 会自动等于 parent
    parent: Option<i64>,
    /// 楼中楼回复的根评论 rpid；单独提供无效，必须与 parent 同时传
    root: Option<i64>,
    /// 需要真实 @ 的用户；正文里必须同时写 @昵称
    mentions: Option<Vec<Mention>>
});

fn tool_with<P: JsonSchema>(
    name: &'static str,
    description: &'static str,
    read_only: bool,
    idempotent: bool,
) -> Tool {
    let schema = serde_json::to_value(schemars::schema_for!(P)).expect("schema serialization");
    Tool::new(name, description, schema.as_object().unwrap().clone()).with_annotations(
        ToolAnnotations::from_raw(
            None,
            Some(read_only),
            Some(false),
            Some(idempotent),
            Some(name != "get_login_status"),
        ),
    )
}

fn tool<P: JsonSchema>(name: &'static str, description: &'static str) -> Tool {
    tool_with::<P>(name, description, true, true)
}

fn write_tool<P: JsonSchema>(
    name: &'static str,
    description: &'static str,
    idempotent: bool,
) -> Tool {
    tool_with::<P>(name, description, false, idempotent)
}

/// Explicit registry shared by the CLI and both MCP transports.
/// Interaction tools are only included when the operator opts in.
pub fn tools(allow_write: bool, allow_comment: bool) -> Vec<Tool> {
    static READ: OnceLock<Vec<Tool>> = OnceLock::new();
    static WRITE: OnceLock<Vec<Tool>> = OnceLock::new();
    static COMMENT: OnceLock<Vec<Tool>> = OnceLock::new();
    let mut catalog = READ.get_or_init(|| vec![
        tool::<Empty>("get_login_status", "查看服务器是否配置B站Cookie；configured表示已提供凭据，不表示已联网验证。不会返回Cookie。"),
        tool::<Empty>("get_current_user", "联网验证B站登录态，返回当前账号资料；需要Cookie。"),
        tool::<Link>("parse_url", "解析B站链接/BV/av标识；短链接会联网解析。返回aid、bvid、分P与番剧标识。"),
        tool::<Link>("parse_video", "查询视频/番剧详情、简介、UP主、统计和分P cid。不会提交下载或写入历史。"),
        tool::<Search>("search_videos", "搜索视频、UP主、番剧或影视，需要Cookie。search_type: video/bili_user/media_bangumi/media_ft；page从1开始。order: totalrank/pubdate/click/stow/dm；duration: 0/1/2/3/4。"),
        tool::<Limit>("get_hot_search", "查询B站实时热搜榜，limit默认20，最多50。"),
        tool::<Suggest>("get_search_suggest", "根据term查询搜索联想词。"),
        tool::<Bvid>("get_related_videos", "查询视频的相关推荐。"),
        tool::<Comments>("get_video_comments", "分页查询评论区；sort为hot或time。"),
        tool::<Replies>("get_comment_replies", "分页查询评论的楼中楼，root_rpid来自评论列表。"),
        tool::<AiSummary>("get_ai_summary", "查询B站官方AI摘要及章节大纲，需要Cookie。cid和up_mid来自parse_video。无总结时返回null。"),
        tool::<Part>("get_video_subtitles", "查询分P的人工与AI字幕轨道，需要Cookie；subtitle_url用于读取正文。"),
        tool::<Subtitle>("get_subtitle_content", "读取字幕正文及时间轴，支持from_seconds和limit；仅接收B站字幕CDN URL。"),
        tool::<Danmaku>("get_video_danmaku", "查询弹幕时间轴，需要Cookie和视频时长；最多返回2000条，不拉取历史弹幕。"),
        tool::<Article>("get_article_content", "查询专栏或opus图文正文；cvid与opus_id二选一，opus_id传字符串。"),
        tool::<Mid>("get_upper_info", "查询UP主资料、签名、粉丝数和投稿数。"),
        tool::<UpperVideos>("get_upper_videos", "分页查询UP主投稿，需要Cookie，支持keyword筛选。"),
        tool::<Mid>("get_upper_collections", "列出UP主的合集和视频列表。"),
        tool::<Collection>("get_collection_videos", "分页查询season或series合集内容，需要Cookie。"),
        tool::<Ranking>("get_ranking", "查询全站或分区排行榜，rid默认0。"),
        tool::<Region>("get_region_videos", "查询分区视频，rid默认1，page_size默认20、最多50。"),
        tool::<PgcRank>("get_pgc_rank", "查询番剧/国创/影视榜，season_type默认4。"),
        tool::<Empty>("get_weekly_series", "查询每周必看期数列表。"),
        tool::<Weekly>("get_weekly_detail", "查询每周必看某期内容；number不传为最新一期。"),
        tool::<Empty>("get_precious_list", "查询入站必刷官方精选。"),
        tool::<Recommend>("get_recommend", "查询推荐视频，fresh_idx可递增换一批。"),
        tool::<Dynamic>("get_dynamic_feed", "查询关注动态，需要Cookie；offset使用上次返回的游标。"),
        tool::<History>("get_watch_history", "查询账号观看历史，需要Cookie；cursor需整体原样传回。"),
        tool::<Empty>("get_favorite_folders", "查询当前账号收藏夹，需要Cookie。"),
        tool::<Favorite>("get_favorite_videos", "分页查询收藏夹内容，media_id来自收藏夹列表。"),
        tool::<Empty>("get_watch_later", "查询稍后再看，需要Cookie。"),
        tool::<Page>("get_followings", "分页查询当前账号关注的UP主，需要Cookie。"),
        tool::<Followers>("get_followers", "分页查询用户粉丝，需要Cookie。"),
        tool::<Bangumi>("get_bangumi_follow", "查询追番/追剧，需要Cookie；follow_type为1或2。"),
        tool::<Empty>("get_subscribed_collections", "查询账号收藏的合集，需要Cookie。"),
    ]).clone();
    if allow_write {
        catalog.extend(WRITE.get_or_init(|| vec![
            write_tool::<Like>(
                "like_video",
                "给视频点赞或取消点赞（真实账号操作，点赞列表他人可见）；like默认true，false为取消。需要包含bili_jct的Cookie，且服务器必须显式开启互动写入。",
                true,
            ),
            write_tool::<Coin>(
                "coin_video",
                "给视频投币，会消耗账号真实硬币；multiply为1或2，like_too为同时点赞。需要包含bili_jct的Cookie，且服务器必须显式开启互动写入。",
                false,
            ),
            write_tool::<FavoriteVideo>(
                "favorite_video",
                "把视频加入指定收藏夹；folder_id来自get_favorite_folders。这是真实账号操作，需要包含bili_jct的Cookie，且服务器必须显式开启互动写入。",
                true,
            ),
            write_tool::<WatchLaterAdd>(
                "add_watch_later",
                "把视频加入稍后再看。需要包含bili_jct的Cookie，且服务器必须显式开启互动写入。",
                true,
            ),
            write_tool::<WatchLaterRemove>(
                "remove_watch_later",
                "把视频从稍后再看移除。需要包含bili_jct的Cookie，且服务器必须显式开启互动写入。",
                true,
            ),
        ]).clone());
    }
    if allow_comment {
        catalog.extend(COMMENT.get_or_init(|| vec![
            write_tool::<PostComment>(
                "post_comment",
                "在视频评论区发表评论或回复（真实公开操作：会以账号身份发言，回复和@会给对方发通知）。message最多1000字符；回复评论传parent=rpid，楼中楼再传root；mentions用mid+name把正文里的@昵称变成真实提及。需要包含bili_jct的Cookie，且服务器必须显式开启评论写入。",
                false,
            ),
        ]).clone());
    }
    catalog
}

pub struct QueryService {
    credential: RwLock<Option<Credential>>,
    settings: RwLock<RuntimeSettings>,
    startup_concurrency: usize,
    next_request: Mutex<Instant>,
    permits: Semaphore,
    timeout: RwLock<Duration>,
}

impl QueryService {
    pub fn new(credential: Option<Credential>, settings: RuntimeSettings) -> Arc<Self> {
        Self::build(credential, settings, None)
    }

    #[cfg(test)]
    fn new_with_timeout(
        credential: Option<Credential>,
        settings: RuntimeSettings,
        timeout: Duration,
    ) -> Arc<Self> {
        Self::build(credential, settings, Some(timeout))
    }

    fn build(
        credential: Option<Credential>,
        settings: RuntimeSettings,
        timeout: Option<Duration>,
    ) -> Arc<Self> {
        let startup_concurrency = settings.max_concurrency.max(1) as usize;
        let timeout = timeout.unwrap_or_else(|| Duration::from_secs(settings.timeout_secs.max(1)));
        Arc::new(Self {
            credential: RwLock::new(credential),
            settings: RwLock::new(settings),
            startup_concurrency,
            next_request: Mutex::new(Instant::now()),
            permits: Semaphore::new(startup_concurrency),
            timeout: RwLock::new(timeout),
        })
    }

    pub fn settings(&self) -> RuntimeSettings {
        self.settings.read().expect("settings lock").clone()
    }

    pub fn allow_write(&self) -> bool {
        self.settings.read().expect("settings lock").allow_write
    }

    pub fn allow_comment(&self) -> bool {
        self.settings.read().expect("settings lock").allow_comment
    }

    pub fn restart_required(&self) -> bool {
        self.settings.read().expect("settings lock").max_concurrency as usize
            != self.startup_concurrency
    }

    pub fn update_settings(&self, settings: RuntimeSettings) -> Result<()> {
        settings.validate()?;
        *self.timeout.write().expect("timeout lock") = Duration::from_secs(settings.timeout_secs);
        *self.settings.write().expect("settings lock") = settings;
        Ok(())
    }

    pub fn update_credential(&self, credential: Option<Credential>) {
        *self.credential.write().expect("credential lock") = credential;
    }

    pub fn credential_snapshot(&self) -> Option<Credential> {
        self.credential.read().expect("credential lock").clone()
    }

    pub fn tools(&self) -> Vec<Tool> {
        tools(self.allow_write(), self.allow_comment())
    }

    fn credential(&self) -> Result<Credential> {
        self.credential_snapshot().ok_or_else(|| {
            QueryError::new(
                "AUTH_REQUIRED",
                "此查询需要B站Cookie；请由服务器管理员配置 --cookie-file、WEIYU_COOKIE_FILE 或通过管理台扫码登录。",
            )
        })
    }

    fn ensure_write(&self) -> Result<()> {
        if self.allow_write() {
            Ok(())
        } else {
            Err(QueryError::new(
                "UNKNOWN_TOOL",
                "互动写入工具未启用；请在管理台开启写入，或启动时设置 WEIYU_ALLOW_WRITE=true。",
            ))
        }
    }

    /// 评论写入是独立开关：即使 allow_write=true 也不会自动开放发评论。
    fn ensure_comment(&self) -> Result<()> {
        if self.allow_comment() {
            Ok(())
        } else {
            Err(QueryError::new(
                "UNKNOWN_TOOL",
                "评论工具未启用；请在管理台开启评论写入，或启动时设置 WEIYU_ALLOW_COMMENT=true。",
            ))
        }
    }

    /// 互动写入额外要求 CSRF 字段；只有 SESSDATA 的 Cookie 只能读。
    fn write_credential(&self) -> Result<Credential> {
        let credential = self.credential()?;
        if credential.bili_jct.trim().is_empty() {
            return Err(QueryError::new(
                "AUTH_REQUIRED",
                "互动写入需要包含 bili_jct 的 Cookie；请通过管理台重新扫码登录。",
            ));
        }
        Ok(credential)
    }

    pub async fn call(&self, name: &str, arguments: Value) -> Result<Value> {
        if !self.tools().iter().any(|tool| tool.name == name) {
            return Err(QueryError::new("UNKNOWN_TOOL", format!("未知工具：{name}")));
        }
        let _permit = self
            .permits
            .try_acquire()
            .map_err(|_| QueryError::new("BUSY", "查询并发已满，请稍后重试"))?;
        let settings = self.settings();
        let interval = if settings.rate_limit == 0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(1.0 / settings.rate_limit as f64)
        };
        let timeout = *self.timeout.read().expect("timeout lock");
        tokio::time::timeout(timeout, async {
            if name != "get_login_status" && interval != Duration::ZERO {
                let mut next = self.next_request.lock().await;
                tokio::time::sleep_until(*next).await;
                *next = Instant::now() + interval;
            }
            let result = self.execute(name, arguments).await?;
            if serde_json::to_vec(&result)?.len() > 2 * 1024 * 1024 {
                return Err(QueryError::new(
                    "RESULT_TOO_LARGE",
                    "结果超过2MiB，请缩小查询范围",
                ));
            }
            Ok(result)
        })
        .await
        .map_err(|_| QueryError::new("TIMEOUT", "查询超时，请稍后重试或缩小范围"))?
    }
    async fn execute(&self, name: &str, args: Value) -> Result<Value> {
        let credential_owned = self.credential_snapshot();
        let credential = credential_owned.as_ref();
        match name {
            "get_login_status" => {
                let _: Empty = parse(args)?;
                let write_ready = credential.is_some_and(|c| !c.bili_jct.trim().is_empty());
                Ok(json!({
                    "configured": credential.is_some(),
                    "verified": false,
                    "write_enabled": self.allow_write(),
                    "comment_enabled": self.allow_comment(),
                    "write_ready": write_ready,
                    "hint": "使用get_current_user联网验证登录态；互动写入和评论写入都需要服务器显式开启且Cookie包含bili_jct"
                }))
            }
            "get_current_user" => {
                let _: Empty = parse(args)?;
                output(bili::passport::validate_credentials(&self.credential()?).await?)
            }
            "parse_url" | "parse_video" => {
                let p: Link = parse(args)?;
                let parsed = input::parse_link(&p.url).await?;
                if name == "parse_url" {
                    return output(parsed);
                }
                output(
                    bili::video::get_video_info(
                        parsed.bvid.as_deref(),
                        parsed.aid,
                        parsed.ep_id,
                        parsed.season_id,
                        parsed.media_id,
                        credential,
                    )
                    .await?,
                )
            }
            "search_videos" => {
                let p: Search = parse(args)?;
                input::text(&p.keyword, 200, "keyword")?;
                let kind = choice(
                    p.search_type.as_deref().unwrap_or("video"),
                    &["video", "bili_user", "media_bangumi", "media_ft"],
                    "search_type",
                )?;
                let order = choice(
                    p.order.as_deref().unwrap_or("totalrank"),
                    &["totalrank", "pubdate", "click", "stow", "dm"],
                    "order",
                )?;
                let duration = choice(
                    p.duration.as_deref().unwrap_or("0"),
                    &["0", "1", "2", "3", "4"],
                    "duration",
                )?;
                let tids = p.tids.as_deref().unwrap_or("0");
                if tids.len() > 100 || !tids.chars().all(|c| c.is_ascii_digit() || c == ',') {
                    return Err(QueryError::invalid("tids需为数字分区ID，以逗号分隔"));
                }
                let mut res = bili::search::search(
                    &p.keyword,
                    kind,
                    page(p.page)?,
                    &self.credential()?,
                    order,
                    duration,
                    tids,
                )
                .await?;
                for row in &mut res.results {
                    row.title = plain_text(&row.title);
                    row.description = plain_text(&row.description);
                }
                output(res)
            }
            "get_hot_search" => {
                let p: Limit = parse(args)?;
                output(
                    bili::search::get_hot_search(
                        input::bounded(p.limit.unwrap_or(20), 1, 50, "limit")?,
                        credential,
                    )
                    .await?,
                )
            }
            "get_search_suggest" => {
                let p: Suggest = parse(args)?;
                input::text(&p.term, 100, "term")?;
                output(bili::search::get_suggest(&p.term).await?)
            }
            "get_related_videos" => {
                let p: Bvid = parse(args)?;
                input::validate_bvid(&p.bvid)?;
                output(bili::video::get_related_videos(&p.bvid, credential).await?)
            }
            "get_video_comments" => {
                let p: Comments = parse(args)?;
                let aid = input::validate_bvid(&p.bvid)?;
                let sort = choice(p.sort.as_deref().unwrap_or("hot"), &["hot", "time"], "sort")?;
                output(
                    bili::comment::get_comments(
                        aid as i64,
                        page(p.page)?,
                        if sort == "time" { 2 } else { 3 },
                        credential,
                    )
                    .await?,
                )
            }
            "get_comment_replies" => {
                let p: Replies = parse(args)?;
                let aid = input::validate_bvid(&p.bvid)?;
                output(
                    bili::comment::get_replies(
                        aid as i64,
                        input::positive(p.root_rpid, "root_rpid")?,
                        page(p.page)?,
                        credential,
                    )
                    .await?,
                )
            }
            "get_ai_summary" => {
                let p: AiSummary = parse(args)?;
                input::validate_bvid(&p.bvid)?;
                output(
                    bili::video::get_ai_summary(
                        &p.bvid,
                        input::positive(p.cid, "cid")?,
                        input::positive(p.up_mid, "up_mid")? as u64,
                        &self.credential()?,
                    )
                    .await?,
                )
            }
            "get_video_subtitles" => {
                let p: Part = parse(args)?;
                let aid = input::validate_bvid(&p.bvid)?;
                let tracks = bili::subtitle::get_subtitle_urls_all(
                    &bili::api_client(),
                    &self.credential()?,
                    input::positive(p.cid, "cid")?,
                    &p.bvid,
                    aid,
                )
                .await?;
                output(tracks.iter().map(|t|json!({"lan":t.lan,"lan_doc":t.lan_doc,"is_ai":t.is_ai_sub(),"subtitle_url":bili::http_to_https(&t.subtitle_url)})).collect::<Vec<_>>())
            }
            "get_subtitle_content" => {
                let p: Subtitle = parse(args)?;
                let target = input::subtitle_url(&p.subtitle_url)?;
                let limit = input::bounded(p.limit.unwrap_or(1000), 1, 5000, "limit")? as usize;
                let from = p.from_seconds.unwrap_or(0.0);
                if !from.is_finite() || from < 0.0 {
                    return Err(QueryError::invalid("from_seconds必须为非负数"));
                }
                let cues = bili::subtitle::fetch_subtitle_cues_bounded(
                    &input::restricted_client(true)?,
                    target.as_str(),
                    4 * 1024 * 1024,
                )
                .await?;
                let total = cues.len();
                let selected: Vec<_> = cues.into_iter().filter(|cue| cue.to >= from).collect();
                let available = selected.len();
                Ok(
                    json!({"total":total,"returned":available.min(limit),"has_more":available>limit,"cues":selected.into_iter().take(limit).collect::<Vec<_>>()}),
                )
            }
            "get_video_danmaku" => {
                let p: Danmaku = parse(args)?;
                let aid = input::validate_bvid(&p.bvid)?;
                let duration = input::bounded(p.duration_seconds, 1, 21600, "duration_seconds")?;
                let limit = input::bounded(p.limit.unwrap_or(300), 1, 2000, "limit")? as usize;
                let rows = bili::danmaku::fetch_danmaku_list(
                    &bili::api_client(),
                    &self.credential()?,
                    input::positive(p.cid, "cid")?,
                    aid,
                    duration as u64,
                    0,
                )
                .await?;
                Ok(
                    json!({"total":rows.len(),"returned":rows.len().min(limit),"danmaku":rows.iter().take(limit).map(|d|json!({"time_seconds":d.timeline_s,"text":d.content})).collect::<Vec<_>>()}),
                )
            }
            "get_article_content" => {
                let p: Article = parse(args)?;
                let data = match (p.cvid, p.opus_id) {
                    (Some(id), None) if id > 0 => {
                        bili::article::get_article(id, credential).await?
                    }
                    (None, Some(id))
                        if !id.is_empty()
                            && id.len() <= 20
                            && id.chars().all(|c| c.is_ascii_digit()) =>
                    {
                        bili::article::get_opus(&id, credential).await?
                    }
                    _ => return Err(QueryError::invalid("cvid与opus_id必须提供其中一个有效ID")),
                };
                let text = plain_text(&data.content_html);
                let truncated = text.chars().count() > 50000;
                Ok(
                    json!({"cvid":data.cvid,"opus_id":data.opus_id,"title":data.title,"author":data.author_name,"publish_time":data.publish_time,"content":text.chars().take(50000).collect::<String>(),"truncated":truncated}),
                )
            }
            "get_upper_info" => {
                let p: Mid = parse(args)?;
                output(
                    bili::submission::get_upper_info(input::positive(p.mid, "mid")?, credential)
                        .await?,
                )
            }
            "get_upper_videos" => {
                let p: UpperVideos = parse(args)?;
                if let Some(k) = &p.keyword {
                    input::text(k, 200, "keyword")?;
                }
                output(
                    bili::submission::get_submission_videos(
                        input::positive(p.mid, "mid")?,
                        page(p.page)?,
                        p.keyword.as_deref(),
                        &self.credential()?,
                    )
                    .await?,
                )
            }
            "get_upper_collections" => {
                let p: Mid = parse(args)?;
                output(
                    bili::collection::get_upper_collections(
                        input::positive(p.mid, "mid")?,
                        credential,
                    )
                    .await?,
                )
            }
            "get_collection_videos" => {
                let p: Collection = parse(args)?;
                input::text(&p.sid, 20, "sid")?;
                if !p.sid.chars().all(|c| c.is_ascii_digit()) {
                    return Err(QueryError::invalid("sid需为数字ID字符串"));
                }
                output(
                    bili::collection::get_collection_videos(
                        input::positive(p.mid, "mid")?,
                        &p.sid,
                        choice(&p.collection_type, &["season", "series"], "collection_type")?,
                        page(p.page)?,
                        &self.credential()?,
                    )
                    .await?,
                )
            }
            "get_ranking" => {
                let p: Ranking = parse(args)?;
                output(
                    bili::ranking::get_ranking(
                        input::bounded(p.rid.unwrap_or(0), 0, 10000, "rid")?,
                        credential,
                    )
                    .await?,
                )
            }
            "get_region_videos" => {
                let p: Region = parse(args)?;
                output(
                    bili::region::get_region(
                        input::bounded(p.rid.unwrap_or(1), 1, 10000, "rid")?,
                        input::bounded(p.page_size.unwrap_or(20), 1, 50, "page_size")?,
                        credential,
                    )
                    .await?,
                )
            }
            "get_pgc_rank" => {
                let p: PgcRank = parse(args)?;
                output(
                    bili::pgc::get_pgc_rank(
                        input::bounded(p.season_type.unwrap_or(4) as u32, 1, 7, "season_type")?
                            as u8,
                        credential,
                    )
                    .await?,
                )
            }
            "get_weekly_series" => {
                let _: Empty = parse(args)?;
                output(bili::weekly::get_weekly_series_list(credential).await?)
            }
            "get_weekly_detail" => {
                let p: Weekly = parse(args)?;
                if let Some(n) = p.number {
                    input::positive(n, "number")?;
                }
                output(bili::weekly::get_weekly_detail(p.number, credential).await?)
            }
            "get_precious_list" => {
                let _: Empty = parse(args)?;
                output(bili::weekly::get_precious_list(credential).await?)
            }
            "get_recommend" => {
                let p: Recommend = parse(args)?;
                output(
                    bili::recommend::get_recommend(
                        input::bounded(p.fresh_idx.unwrap_or(1), 1, 10000, "fresh_idx")?,
                        credential,
                    )
                    .await?,
                )
            }
            "get_dynamic_feed" => {
                let p: Dynamic = parse(args)?;
                let offset = p.offset.unwrap_or_default();
                if offset.len() > 200 {
                    return Err(QueryError::invalid("offset过长"));
                }
                output(bili::dynamic::get_dynamic_feed(&offset, &self.credential()?).await?)
            }
            "get_watch_history" => {
                let p: History = parse(args)?;
                let cursor = if let Some(c) = p.cursor {
                    input::positive(c.view_at, "cursor.view_at")?;
                    if c.max.as_ref().is_some_and(|s| s.len() > 100)
                        || c.business.as_ref().is_some_and(|s| s.len() > 100)
                    {
                        return Err(QueryError::invalid("cursor字段过长"));
                    }
                    Some(bili::history::HistoryCursor {
                        view_at: c.view_at,
                        business: c.business.unwrap_or_default(),
                        max: c.max.unwrap_or_default(),
                    })
                } else {
                    None
                };
                output(bili::history::get_watch_history(cursor, &self.credential()?).await?)
            }
            "get_favorite_folders" => {
                let _: Empty = parse(args)?;
                output(bili::favorite::get_user_favorite_folders(&self.credential()?).await?)
            }
            "get_favorite_videos" => {
                let p: Favorite = parse(args)?;
                input::text(&p.media_id, 20, "media_id")?;
                if !p.media_id.chars().all(|c| c.is_ascii_digit()) {
                    return Err(QueryError::invalid("media_id需为数字ID字符串"));
                }
                output(
                    bili::favorite::get_favorite_videos(&p.media_id, page(p.page)?, credential)
                        .await?,
                )
            }
            "get_watch_later" => {
                let _: Empty = parse(args)?;
                output(bili::watch_later::get_watch_later(&self.credential()?).await?)
            }
            "get_followings" => {
                let p: Page = parse(args)?;
                output(bili::following::get_followings(page(p.page)?, &self.credential()?).await?)
            }
            "get_followers" => {
                let p: Followers = parse(args)?;
                output(
                    bili::following::get_followers(
                        input::positive(p.mid, "mid")?,
                        page(p.page)?,
                        &self.credential()?,
                    )
                    .await?,
                )
            }
            "get_bangumi_follow" => {
                let p: Bangumi = parse(args)?;
                output(
                    bili::pgc::get_bangumi_follow(
                        input::bounded(p.follow_type.unwrap_or(1) as u32, 1, 2, "follow_type")?
                            as u8,
                        page(p.page)?,
                        &self.credential()?,
                    )
                    .await?,
                )
            }
            "get_subscribed_collections" => {
                let _: Empty = parse(args)?;
                output(bili::collection::get_subscribed_collections(&self.credential()?).await?)
            }
            "like_video" => {
                self.ensure_write()?;
                let p: Like = parse(args)?;
                input::validate_bvid(&p.bvid)?;
                let liked = p.like.unwrap_or(true);
                bili::interaction::like_video(
                    &self.write_credential()?,
                    &p.bvid,
                    if liked { 1 } else { 2 },
                )
                .await?;
                Ok(json!({"ok":true,"bvid":p.bvid,"liked":liked}))
            }
            "coin_video" => {
                self.ensure_write()?;
                let p: Coin = parse(args)?;
                input::validate_bvid(&p.bvid)?;
                let multiply =
                    input::bounded(p.multiply.unwrap_or(1) as u32, 1, 2, "multiply")? as u8;
                let like_too = p.like_too.unwrap_or(false);
                bili::interaction::coin_video(
                    &self.write_credential()?,
                    &p.bvid,
                    multiply,
                    if like_too { 1 } else { 0 },
                )
                .await?;
                Ok(json!({"ok":true,"bvid":p.bvid,"multiply":multiply,"like_too":like_too}))
            }
            "favorite_video" => {
                self.ensure_write()?;
                let p: FavoriteVideo = parse(args)?;
                let aid = input::validate_bvid(&p.bvid)? as i64;
                let folder_id = input::positive(p.folder_id, "folder_id")?;
                bili::interaction::favorite_video(&self.write_credential()?, aid, folder_id)
                    .await?;
                Ok(json!({"ok":true,"bvid":p.bvid,"aid":aid,"folder_id":folder_id}))
            }
            "add_watch_later" => {
                self.ensure_write()?;
                let p: WatchLaterAdd = parse(args)?;
                input::validate_bvid(&p.bvid)?;
                bili::watch_later::add_watch_later(&p.bvid, &self.write_credential()?).await?;
                Ok(json!({"ok":true,"bvid":p.bvid}))
            }
            "remove_watch_later" => {
                self.ensure_write()?;
                let p: WatchLaterRemove = parse(args)?;
                input::validate_bvid(&p.bvid)?;
                bili::watch_later::remove_watch_later(&p.bvid, &self.write_credential()?).await?;
                Ok(json!({"ok":true,"bvid":p.bvid}))
            }
            "post_comment" => {
                self.ensure_comment()?;
                let p: PostComment = parse(args)?;
                let aid = input::validate_bvid(&p.bvid)? as i64;
                input::text(&p.message, 1000, "message")?;
                let mentions: Vec<(String, i64)> = p
                    .mentions
                    .unwrap_or_default()
                    .into_iter()
                    .map(|m| (m.name.trim().to_string(), m.mid))
                    .collect();
                let posted = bili::interaction::post_comment(
                    &self.write_credential()?,
                    aid,
                    &p.message,
                    p.root,
                    p.parent,
                    &mentions,
                )
                .await?;
                Ok(json!({
                    "ok": true,
                    "bvid": p.bvid,
                    "aid": aid,
                    "rpid": posted.rpid,
                    "root": posted.root,
                    "parent": posted.parent,
                    "mentions": mentions.len(),
                }))
            }
            _ => Err(QueryError::new("UNKNOWN_TOOL", "工具不存在")),
        }
    }
}

fn parse<P: DeserializeOwned>(value: Value) -> Result<P> {
    Ok(serde_json::from_value(value)?)
}
fn output<T: Serialize>(value: T) -> Result<Value> {
    Ok(serde_json::to_value(value)?)
}
fn page(value: Option<u32>) -> Result<u32> {
    input::bounded(value.unwrap_or(1), 1, 10000, "page")
}
fn choice<'a>(value: &'a str, choices: &[&str], name: &str) -> Result<&'a str> {
    if !choices.contains(&value) {
        return Err(QueryError::invalid(format!(
            "{name}可选值：{}",
            choices.join(",")
        )));
    }
    Ok(value)
}

fn plain_text(value: &str) -> String {
    let value = value
        .replace("</p>", "\n\n")
        .replace("<br>", "\n")
        .replace("<br/>", "\n")
        .replace("<br />", "\n");
    let mut tag = false;
    let mut result = String::new();
    for ch in value.chars() {
        match ch {
            '<' => tag = true,
            '>' => tag = false,
            _ if !tag => result.push(ch),
            _ => {}
        }
    }
    result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn queued_queries_timeout_without_leaking_concurrency() {
        let service = QueryService::new_with_timeout(
            None,
            RuntimeSettings {
                rate_limit: 1,
                max_concurrency: 1,
                timeout_secs: 1,
                allow_write: false,
                allow_comment: false,
            },
            Duration::from_millis(50),
        );
        service
            .call("parse_url", json!({"url":"BV17x411w7KC"}))
            .await
            .unwrap();
        let error = service
            .call("parse_url", json!({"url":"BV17x411w7KC"}))
            .await
            .unwrap_err();
        assert_eq!(error.code, "TIMEOUT");
        assert_eq!(
            service.call("get_login_status", json!({})).await.unwrap()["configured"],
            false
        );
    }
    #[test]
    fn write_tools_are_hidden_unless_explicitly_enabled() {
        let read = tools(false, false);
        assert_eq!(read.len(), 35);
        for name in [
            "like_video",
            "coin_video",
            "favorite_video",
            "add_watch_later",
            "remove_watch_later",
            "post_comment",
        ] {
            assert!(read.iter().all(|tool| tool.name != name));
        }
        let write = tools(true, false);
        assert_eq!(write.len(), 40);
        for name in [
            "like_video",
            "coin_video",
            "favorite_video",
            "add_watch_later",
            "remove_watch_later",
        ] {
            assert!(write.iter().any(|tool| tool.name == name));
        }
        assert!(write.iter().all(|tool| tool.name != "post_comment"));

        let comment = tools(false, true);
        assert_eq!(comment.len(), 36);
        assert!(comment.iter().any(|tool| tool.name == "post_comment"));
        assert!(comment.iter().all(|tool| tool.name != "like_video"));

        assert_eq!(tools(true, true).len(), 41);
    }

    #[tokio::test]
    async fn write_tools_require_opt_in_and_credentials() {
        let service = QueryService::new(
            None,
            RuntimeSettings {
                rate_limit: 0,
                max_concurrency: 4,
                timeout_secs: 1,
                allow_write: true,
                allow_comment: false,
            },
        );
        assert!(service.allow_write());
        assert_eq!(
            service
                .call("like_video", json!({"bvid":"BV17x411w7KC"}))
                .await
                .unwrap_err()
                .code,
            "AUTH_REQUIRED"
        );
        assert_eq!(
            service
                .call("add_watch_later", json!({"bvid":"BV17x411w7KC"}))
                .await
                .unwrap_err()
                .code,
            "AUTH_REQUIRED"
        );
    }

    #[test]
    fn settings_update_is_hot_except_concurrency() {
        let service = QueryService::new(
            None,
            RuntimeSettings {
                rate_limit: 2,
                max_concurrency: 4,
                timeout_secs: 45,
                allow_write: false,
                allow_comment: false,
            },
        );
        assert!(!service.restart_required());
        let mut settings = service.settings();
        settings.allow_write = true;
        settings.rate_limit = 5;
        service.update_settings(settings).unwrap();
        assert!(service.allow_write());
        assert_eq!(service.tools().len(), 40);
        let mut settings = service.settings();
        settings.allow_comment = true;
        service.update_settings(settings).unwrap();
        assert!(service.allow_comment());
        assert_eq!(service.tools().len(), 41);
        let mut settings = service.settings();
        settings.max_concurrency = 8;
        service.update_settings(settings).unwrap();
        assert!(service.restart_required());
    }

    #[tokio::test]
    async fn comment_tool_is_separately_gated_and_validates_before_network() {
        let service = QueryService::new(None, RuntimeSettings::default());
        assert_eq!(
            service
                .call(
                    "post_comment",
                    json!({"bvid":"BV17x411w7KC","message":"hi"})
                )
                .await
                .unwrap_err()
                .code,
            "UNKNOWN_TOOL"
        );

        let mut settings = service.settings();
        settings.allow_comment = true;
        service.update_settings(settings).unwrap();
        for args in [
            json!({"bvid":"invalid","message":"hi"}),
            json!({"bvid":"BV17x411w7KC","message":"   "}),
            json!({"bvid":"BV17x411w7KC","message":"字".repeat(1001)}),
        ] {
            assert_eq!(
                service.call("post_comment", args).await.unwrap_err().code,
                "INVALID_PARAMS"
            );
        }
        assert_eq!(
            service
                .call(
                    "post_comment",
                    json!({"bvid":"BV17x411w7KC","message":"hi"})
                )
                .await
                .unwrap_err()
                .code,
            "AUTH_REQUIRED"
        );
    }

    #[tokio::test]
    async fn invalid_arguments_are_rejected_before_network() {
        let service = QueryService::new(
            None,
            RuntimeSettings {
                rate_limit: 0,
                max_concurrency: 4,
                timeout_secs: 1,
                allow_write: false,
                allow_comment: false,
            },
        );
        for (name, args) in [
            ("search_videos", json!({"keyword":"test","page":0})),
            ("get_video_comments", json!({"bvid":"invalid"})),
            ("get_article_content", json!({"cvid":1,"opus_id":"123"})),
            (
                "get_login_status",
                json!({"cookie":"do-not-accept-secrets"}),
            ),
        ] {
            assert_eq!(
                service.call(name, args).await.unwrap_err().code,
                "INVALID_PARAMS"
            );
        }
        assert_eq!(
            service
                .call("coin_video", json!({}))
                .await
                .unwrap_err()
                .code,
            "UNKNOWN_TOOL"
        );
    }
}
