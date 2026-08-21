# 登录页改版实现计划

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 将 foxops-web 登录页改为全屏背景视频 + 居中登录卡。

**Tech Stack:** Vue 3、Element Plus、现有 auth API

---

### Task 1: 资源目录与页面改写

**Files:**
- Create: `foxops-web/public/videos/README.md`
- Modify: `foxops-web/src/views/auth/login/index.vue`
- Modify: `foxops-web/src/views/auth/login/style.css`

**Steps:**
1. 创建 videos 目录说明
2. 重写登录页布局与样式
3. 手动打开 `/auth/login` 验收
