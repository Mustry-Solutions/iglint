def onKeyEvent(page, event):
	system.perspective.sendMessage("motor-refresh", payload={}, scope="page", pageId=page.props.pageId)