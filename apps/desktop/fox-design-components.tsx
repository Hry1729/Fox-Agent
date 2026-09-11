import React from 'react'
import {createRoot} from 'react-dom/client'
import './src/styles/globals.css'
import './src/styles/workbench.css'
import './src/styles/workspace-pages.css'
import {Button} from './src/components/ui/button'
import {Badge} from './src/components/ui/badge'
import {Input} from './src/components/ui/input'
import {Textarea} from './src/components/ui/textarea'
import {Switch} from './src/components/ui/switch'
import {Progress} from './src/components/ui/progress'
import {Skeleton} from './src/components/ui/skeleton'
import {Separator} from './src/components/ui/separator'
import {Card,CardHeader,CardTitle,CardDescription,CardContent} from './src/components/ui/card'
import {Tabs,TabsList,TabsTrigger} from './src/components/ui/tabs'
import {Alert,AlertTitle,AlertDescription} from './src/components/ui/alert'
const styles=['default','outline','secondary','ghost','destructive','link'] as const
const sizes=['xs','sm','default','lg'] as const
function App(){return <main style={{padding:32,overflow:'auto',height:'100vh',background:'var(--background)',color:'var(--foreground)'}}><h1>Fox 基础组件校对</h1><p>独立样式样本 · 不加载对话或业务数据</p>{styles.map(variant=><section key={variant} style={{display:'flex',alignItems:'center',gap:20,marginBottom:20}}><strong style={{width:100}}>{variant}</strong>{sizes.map(size=><Button key={size} variant={variant} size={size}>{size} 按钮</Button>)}<Button variant={variant} disabled>禁用</Button><Badge variant={variant}>标签</Badge></section>)}<section style={{display:'grid',gridTemplateColumns:'repeat(3, 1fr)',gap:20}}><Input placeholder="输入内容"/><Input value="已填写内容" readOnly/><Input placeholder="禁用" disabled/><Input placeholder="错误" aria-invalid/><Textarea placeholder="多行内容"/><div><Switch aria-label="开关关闭"/><Switch aria-label="开关开启" defaultChecked/><Switch aria-label="开关禁用" disabled/><Progress value={45}/><Separator/><Skeleton style={{height:28,marginTop:20}}/></div></section></main>}
if(new URLSearchParams(location.search).get('theme')==='dark') document.documentElement.classList.add('dark')
function Extended(){return <div style={{height:'100vh',overflow:'auto',background:'var(--background)',color:'var(--foreground)'}}><App/><section style={{padding:32,display:'grid',gridTemplateColumns:'repeat(2,1fr)',gap:24}}><Card><CardHeader><CardTitle>卡片标题</CardTitle><CardDescription>补充说明用于解释这一组内容。</CardDescription></CardHeader><CardContent>在这里组织同一主题的信息。</CardContent></Card><Card size="sm"><CardHeader><CardTitle>紧凑卡片</CardTitle><CardDescription>补充说明用于解释这一组内容。</CardDescription></CardHeader><CardContent>在这里组织同一主题的信息。</CardContent></Card><Tabs defaultValue="first"><TabsList><TabsTrigger value="first">概览</TabsTrigger><TabsTrigger value="second">设置</TabsTrigger><TabsTrigger value="third" disabled>记录</TabsTrigger></TabsList></Tabs><Tabs defaultValue="first"><TabsList variant="line"><TabsTrigger value="first">概览</TabsTrigger><TabsTrigger value="second">设置</TabsTrigger><TabsTrigger value="third" disabled>记录</TabsTrigger></TabsList></Tabs><Alert><AlertTitle>提示标题</AlertTitle><AlertDescription>说明当前状态与下一步操作。</AlertDescription></Alert><Alert variant="destructive"><AlertTitle>操作未完成</AlertTitle><AlertDescription>请检查输入内容后再试。</AlertDescription></Alert></section></div>}
createRoot(document.getElementById('root')!).render(<Extended/>);
