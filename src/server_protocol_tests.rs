use super::*;

#[tokio::test]
async fn decisions_route_keeps_wire_bytes_and_local_security_guards() {
    let body=br#"{ "model":"gpt-6-luna", "input":"private-image-description", "questions":[{"type":"predicate","instructions":"Check","name":"q"}], "extension":42 }"#.to_vec();
    let expected=body.clone();
    let response=br#"{ "model":"gpt-6-luna", "answers":[{"type":"predicate","name":"q","probability":0.8}], "usage":{"input_tokens":2,"output_tokens":0} }"#.to_vec();
    let output=response.clone();
    let (url,task)=mock(Router::new().route("/v1/decisions",post(move |request:Request| async move {
        assert_eq!(request.headers()[header::AUTHORIZATION],"Bearer test-secret");
        assert_eq!(to_bytes(request.into_body(),4096).await.unwrap().as_ref(),expected);
        output
    }))).await;
    let fixture=fixture_with_config(&url.replace("/v1/systemone","/v1/decisions"),4096,false,|config| {
        config.protocol=model::Protocol::OpenaiDecisions;config.provider=Some("openai".into());config.model_profile=Some("openai-decisions".into());
    });
    let app=router(fixture.state.clone());
    for invalid in ["host","cross-site","credential"] {
        let mut request=native(body.clone());*request.uri_mut()="/v1/decisions".parse().unwrap();
        match invalid {
            "host"=>{request.headers_mut().insert(header::HOST,HeaderValue::from_static("attacker.invalid"));},
            "cross-site"=>{request.headers_mut().insert("sec-fetch-site",HeaderValue::from_static("cross-site"));},
            _=>{request.headers_mut().remove(header::AUTHORIZATION);},
        }
        let denied=app.clone().oneshot(request).await.unwrap();
        assert!(matches!(denied.status(),StatusCode::FORBIDDEN|StatusCode::UNAUTHORIZED));
    }
    let mut request=native(body);*request.uri_mut()="/v1/decisions".parse().unwrap();
    let received=app.oneshot(request).await.unwrap();assert_eq!(received.status(),StatusCode::OK);
    assert_eq!(to_bytes(received.into_body(),4096).await.unwrap().as_ref(),response);
    let record=saved(&fixture).await;
    assert_eq!(record["protocol"],"openai-decisions");assert_eq!(record["answers"][0]["valid"],true);
    assert_eq!(record["answers"][0]["value"],0.8);assert_eq!(record["request_extra"]["extension"],42);
    assert!(!record.to_string().contains("private-image-description"));
    fixture.state.collector.shutdown().await;task.abort();let _=task.await;
}

#[tokio::test]
async fn cloudflare_route_preserves_result_wrapper_for_the_client() {
    let body=br#"{"questions":{"q":{"type":"noul","instructions":"Check"}},"state":"private input"}"#.to_vec();
    let expected=body.clone();
    let output=br#"{"result":{"model":"@cf/cloudflare/clef-omni","answers":{"q":{"type":"noul","noul":0.7}}},"success":true,"errors":[],"messages":[]}"#.to_vec();
    let response=output.clone();
    let(url,task)=mock(Router::new().route("/v1/systemone",post(move |request:Request|async move {
        assert_eq!(to_bytes(request.into_body(),4096).await.unwrap().as_ref(),expected);output
    }))).await;
    let fixture=fixture_with_config(&url,4096,false,|config|config.protocol=model::Protocol::CloudflareSystemone);
    let received=router(fixture.state.clone()).oneshot(native(body)).await.unwrap();assert_eq!(received.status(),StatusCode::OK);
    assert_eq!(to_bytes(received.into_body(),4096).await.unwrap().as_ref(),response);
    let record=saved(&fixture).await;assert_eq!(record["answers"][0]["valid"],true);assert_eq!(record["protocol_metadata"]["success"],true);
    fixture.state.collector.shutdown().await;task.abort();let _=task.await;
}
