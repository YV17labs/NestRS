window.onload = function () {
  window.ui = SwaggerUIBundle({
    url: "api-json",
    dom_id: "#swagger-ui",
    deepLinking: true,
    presets: [SwaggerUIBundle.presets.apis, SwaggerUIStandalonePreset],
    layout: "StandaloneLayout",
    // Swagger UI's default badge sends the document's URL to swagger.io.
    validatorUrl: "none",
  });
};
