from colibri.terminal_qr import render_terminal_qr


def test_render_terminal_qr_outputs_block_qr_for_weixin_payload():
    rendered = render_terminal_qr("https://liteapp.weixin.qq.com/q/7GiQu1?qrcode=4b69ff82f873485e97acae885b11437c&bot_type=3")

    assert rendered is not None
    assert "██" in rendered
    assert len(rendered.splitlines()) == 41


def test_render_terminal_qr_returns_none_for_large_payload():
    assert render_terminal_qr("x" * 200) is None
