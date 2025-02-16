# Borrow Count

> Memory that can be shared with a smart pointer and then reaquired with a future.

## Example

```rs
let unique = Unique::new(0);
let (host, mut share) = unique.share_mut();
tokio::task::spawn(async move {
    tokio::time::sleep(std::time::Duration::from_millis(16)).await;
    *share += 1;
});
let unique = host.await;
assert_eq!(unique.into_inner(), 1)
```