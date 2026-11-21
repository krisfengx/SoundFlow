/**
 * 应用会话令牌：每次应用启动生成一次（模块级，跨路由/页面切换保持）。
 * 用于区分「同一会话内切页导致的任务事件丢失」（可自动续跑）与
 * 「应用重启后 worker 已死」（禁止自动续跑，改为提示手动继续）。
 */
const APP_SESSION_TOKEN = `sess_${Date.now()}_${Math.random().toString(36).slice(2, 10)}`

export default APP_SESSION_TOKEN
