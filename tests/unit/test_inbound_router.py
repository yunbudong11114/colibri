from colibri.inbound_router import InboundRouter


def test_inbound_router_bounds_global_pending_and_orders_per_session():
    router = InboundRouter[str](max_pending=2)
    assert router.try_enqueue("a", "a1")
    assert router.try_enqueue("b", "b1")
    assert not router.try_enqueue("a", "a2")
    assert router.pending_len == 2

    key, item = router.acquire(timeout=0.1)
    assert (key, item) == ("a", "a1")
    assert router.try_enqueue("a", "a2")
    router.release("a")

    first = router.acquire(timeout=0.1)
    second = router.acquire(timeout=0.1)
    assert {first, second} == {("b", "b1"), ("a", "a2")}
    router.release(first[0])
    router.release(second[0])


def test_inbound_router_same_session_not_concurrent():
    router = InboundRouter[str](max_pending=4)
    assert router.try_enqueue("a", "1")
    assert router.try_enqueue("a", "2")
    key, item = router.acquire(timeout=0.1)
    assert (key, item) == ("a", "1")
    assert router.acquire(timeout=0.05) is None
    router.release("a")
    key, item = router.acquire(timeout=0.1)
    assert (key, item) == ("a", "2")
    router.release("a")
